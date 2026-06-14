use std::{collections::HashMap, fs, path::PathBuf, process::Stdio, time::Instant};

use anyhow::{anyhow, Result};
use futures_util::SinkExt;
use rcw_common::{
    protocol::{
        CommandCompletePayload, CommandRequestPayload, DownloadArgs, ErrorCode, ExecArgs,
        KeyboardKeyArgs, KeyboardTypeArgs, MouseClickArgs, MouseMoveArgs, MouseScrollArgs,
        ScreenshotArgs, COMMAND_DOWNLOAD_BEGIN, COMMAND_EXEC, COMMAND_KEYBOARD_KEY,
        COMMAND_KEYBOARD_TYPE, COMMAND_MOUSE_CLICK, COMMAND_MOUSE_MOVE, COMMAND_MOUSE_SCROLL,
        COMMAND_SCREENSHOT, COMMAND_WINDOWS, DEFAULT_SCREENSHOT_FORMAT,
    },
    transfer::{sha256_bytes, BinaryKind},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::watch,
    task::JoinHandle,
};
use tracing::error;

use crate::{
    audit::append_host_audit,
    output::{send_binary_chunks, send_complete, send_error, send_output, SharedWsSink, WsSink},
    platform, HostContext,
};

const PROCESS_OUTPUT_QUEUE_CAPACITY: usize = 128;

pub(crate) type CommandTasks = HashMap<String, CommandTask>;

pub(crate) struct CommandTask {
    pub(crate) session_id: Option<String>,
    cancel_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl CommandTask {
    pub(crate) fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
    }

    pub(crate) fn abort(self) {
        let _ = self.cancel_tx.send(true);
        self.handle.abort();
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

pub(crate) fn prune_finished_command_tasks(command_tasks: &mut CommandTasks) {
    command_tasks.retain(|_, task| !task.is_finished());
}

pub(crate) async fn execute_command(
    context: &HostContext,
    sink: &SharedWsSink,
    command_tasks: &mut CommandTasks,
    message: rcw_common::protocol::WireMessage,
    payload: CommandRequestPayload,
) -> Result<()> {
    if payload.command == COMMAND_EXEC {
        spawn_async_command(
            context.clone(),
            sink.clone(),
            command_tasks,
            message,
            payload,
        )?;
        return Ok(());
    }
    if payload.command == COMMAND_DOWNLOAD_BEGIN {
        spawn_async_command(
            context.clone(),
            sink.clone(),
            command_tasks,
            message,
            payload,
        )?;
        return Ok(());
    }

    let mut sink = sink.lock().await;
    execute_command_inline(context, &mut sink, message, payload).await
}

pub(crate) async fn cancel_command_task(
    context: &HostContext,
    _sink: &SharedWsSink,
    command_tasks: &mut CommandTasks,
    message: rcw_common::protocol::WireMessage,
) -> Result<()> {
    let Some(request_id) = message.request_id.clone() else {
        return Ok(());
    };
    if let Some(task) = command_tasks.get(&request_id) {
        task.cancel();
        append_host_audit(
            context,
            "command.cancel",
            Some(request_id),
            message.session_id,
            None,
            Some("requested"),
        );
    }
    Ok(())
}

async fn execute_command_inline(
    context: &HostContext,
    sink: &mut WsSink,
    message: rcw_common::protocol::WireMessage,
    payload: CommandRequestPayload,
) -> Result<()> {
    let request_id = message
        .request_id
        .clone()
        .ok_or_else(|| anyhow!("command.request missing request_id"))?;
    let session_id = message.session_id.clone();
    let started = Instant::now();

    println!(
        "[{}] {} started request={}",
        rcw_common::audit::now_rfc3339(),
        payload.command,
        request_id
    );
    append_host_audit(
        context,
        "command.started",
        Some(request_id.clone()),
        session_id.clone(),
        Some(payload.command.clone()),
        Some("started"),
    );

    let result = match payload.command.as_str() {
        COMMAND_EXEC => {
            command_exec(sink, &request_id, session_id.clone(), payload.args, None).await
        }
        COMMAND_DOWNLOAD_BEGIN => {
            command_download(sink, &request_id, session_id.clone(), payload.args, None).await
        }
        COMMAND_SCREENSHOT => {
            command_screenshot(sink, &request_id, session_id.clone(), payload.args).await
        }
        COMMAND_WINDOWS => command_windows(sink, &request_id, session_id.clone()).await,
        COMMAND_MOUSE_MOVE => {
            command_mouse_move(sink, &request_id, session_id.clone(), payload.args).await
        }
        COMMAND_MOUSE_CLICK => {
            command_mouse_click(sink, &request_id, session_id.clone(), payload.args).await
        }
        COMMAND_MOUSE_SCROLL => {
            command_mouse_scroll(sink, &request_id, session_id.clone(), payload.args).await
        }
        COMMAND_KEYBOARD_TYPE => {
            command_keyboard_type(sink, &request_id, session_id.clone(), payload.args).await
        }
        COMMAND_KEYBOARD_KEY => {
            command_keyboard_key(sink, &request_id, session_id.clone(), payload.args).await
        }
        _ => Err(anyhow!("unsupported command")),
    };

    let ok = result.is_ok();
    println!(
        "[{}] {} {} request={}",
        rcw_common::audit::now_rfc3339(),
        payload.command,
        if ok { "ok" } else { "failed" },
        request_id
    );
    append_host_audit(
        context,
        "command.complete",
        Some(request_id.clone()),
        session_id.clone(),
        Some(payload.command),
        Some(if ok { "ok" } else { "failed" }),
    );

    if let Err(err) = &result {
        error!(
            "command failed after {} ms: {err}",
            started.elapsed().as_millis()
        );
        let code = error_code_for_command_error(err);
        let _ = send_error(
            sink,
            Some(request_id.clone()),
            session_id.clone(),
            code,
            &err.to_string(),
        )
        .await;
    }
    Ok(())
}

fn spawn_async_command(
    context: HostContext,
    sink: SharedWsSink,
    command_tasks: &mut CommandTasks,
    message: rcw_common::protocol::WireMessage,
    payload: CommandRequestPayload,
) -> Result<()> {
    let request_id = message
        .request_id
        .clone()
        .ok_or_else(|| anyhow!("command.request missing request_id"))?;
    let session_id = message.session_id.clone();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let task_request_id = request_id.clone();
    let task_session_id = session_id.clone();
    let handle = tokio::spawn(async move {
        run_async_command_task(
            context,
            sink,
            task_request_id,
            task_session_id,
            payload,
            cancel_rx,
        )
        .await;
    });
    command_tasks.insert(
        request_id,
        CommandTask {
            session_id,
            cancel_tx,
            handle,
        },
    );
    Ok(())
}

async fn run_async_command_task(
    context: HostContext,
    sink: SharedWsSink,
    request_id: String,
    session_id: Option<String>,
    payload: CommandRequestPayload,
    cancel_rx: watch::Receiver<bool>,
) {
    let started = Instant::now();

    println!(
        "[{}] {} started request={}",
        rcw_common::audit::now_rfc3339(),
        payload.command,
        request_id
    );
    append_host_audit(
        &context,
        "command.started",
        Some(request_id.clone()),
        session_id.clone(),
        Some(payload.command.clone()),
        Some("started"),
    );

    let result = {
        let mut sink = sink.lock().await;
        match payload.command.as_str() {
            COMMAND_EXEC => {
                command_exec(
                    &mut sink,
                    &request_id,
                    session_id.clone(),
                    payload.args,
                    Some(cancel_rx),
                )
                .await
            }
            COMMAND_DOWNLOAD_BEGIN => {
                command_download(
                    &mut sink,
                    &request_id,
                    session_id.clone(),
                    payload.args,
                    Some(cancel_rx),
                )
                .await
            }
            _ => Err(anyhow!("unsupported async command")),
        }
    };

    let ok = result.is_ok();
    println!(
        "[{}] {} {} request={}",
        rcw_common::audit::now_rfc3339(),
        payload.command,
        if ok { "ok" } else { "failed" },
        request_id
    );
    append_host_audit(
        &context,
        "command.complete",
        Some(request_id.clone()),
        session_id.clone(),
        Some(payload.command),
        Some(if ok { "ok" } else { "failed" }),
    );

    if let Err(err) = &result {
        error!(
            "command failed after {} ms: {err}",
            started.elapsed().as_millis()
        );
        let code = error_code_for_command_error(err);
        let _ = send_error(
            &mut *sink.lock().await,
            Some(request_id.clone()),
            session_id.clone(),
            code,
            &err.to_string(),
        )
        .await;
    }
}

fn error_code_for_command_error(err: &anyhow::Error) -> ErrorCode {
    let is_timeout = err
        .chain()
        .any(|cause| cause.to_string().contains("command timed out"));
    if is_timeout {
        return ErrorCode::RequestTimeout;
    }
    let is_cancelled = err
        .chain()
        .any(|cause| cause.to_string().contains("command cancelled"));
    if is_cancelled {
        return ErrorCode::Cancelled;
    }
    let is_unsupported = err.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("only supported on Windows host builds")
            || message.contains("unsupported command")
            || message.contains("only png screenshots are supported")
    });
    if is_unsupported {
        ErrorCode::UnsupportedCommand
    } else {
        ErrorCode::CommandFailed
    }
}

async fn command_exec(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<()> {
    let args: ExecArgs = serde_json::from_value(args)?;
    let started = Instant::now();
    let mut command = Command::new(&args.program);
    command.args(&args.argv);
    if let Some(cwd) = args.cwd {
        command.current_dir(cwd);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.kill_on_drop(true);
    let mut child = command.spawn()?;
    let pid = child.id();
    let (output_tx, mut output_rx) =
        tokio::sync::mpsc::channel::<(String, String)>(PROCESS_OUTPUT_QUEUE_CAPACITY);
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(read_process_stream("stdout", stdout, output_tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(read_process_stream("stderr", stderr, output_tx));
    }

    let wait = child.wait();
    tokio::pin!(wait);
    let mut deadline = args.timeout_ms.map(|timeout_ms| {
        Box::pin(tokio::time::sleep(std::time::Duration::from_millis(
            timeout_ms,
        )))
    });
    let mut cancel_rx = cancel_rx;
    let status = loop {
        tokio::select! {
            Some((stream, data)) = output_rx.recv() => {
                send_output(sink, request_id, session_id.clone(), &stream, &data).await?;
            }
            result = &mut wait => {
                break result?;
            }
            _ = async {
                match deadline.as_mut() {
                    Some(deadline) => deadline.as_mut().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some(pid) = pid {
                    let _ = platform::kill_process_tree(pid);
                }
                return Err(anyhow!("command timed out"));
            }
            changed = async {
                match cancel_rx.as_mut() {
                    Some(cancel_rx) => cancel_rx.changed().await,
                    None => std::future::pending::<Result<(), watch::error::RecvError>>().await,
                }
            } => {
                let cancelled = changed.is_ok()
                    && cancel_rx
                        .as_ref()
                        .map(|cancel_rx| *cancel_rx.borrow())
                        .unwrap_or(false);
                if cancelled {
                    if let Some(pid) = pid {
                        let _ = platform::kill_process_tree(pid);
                    }
                    return Err(anyhow!("command cancelled"));
                }
            }
        }
    };

    while let Ok((stream, data)) = output_rx.try_recv() {
        send_output(sink, request_id, session_id.clone(), &stream, &data).await?;
    }

    send_complete(
        sink,
        request_id,
        session_id,
        CommandCompletePayload {
            ok: status.success(),
            exit_code: status.code(),
            duration_ms: started.elapsed().as_millis() as u64,
            size: None,
            sha256: None,
            summary: Some(format!("program={}", args.program)),
        },
    )
    .await
}

async fn command_download(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<()> {
    let args: DownloadArgs = serde_json::from_value(args)?;
    let path = PathBuf::from(&args.remote_path);
    let size = fs::metadata(&path)?.len();
    let sha256 = send_file_binary_chunks_cancellable(
        sink,
        request_id,
        BinaryKind::DownloadChunk,
        &path,
        size,
        cancel_rx,
    )
    .await?;
    crate::output::send_complete_kind(
        sink,
        rcw_common::protocol::TYPE_DOWNLOAD_COMPLETE,
        request_id,
        session_id,
        CommandCompletePayload {
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
            size: Some(size),
            sha256: Some(sha256),
            summary: Some(format!("read {}", args.remote_path)),
        },
    )
    .await
}

async fn send_file_binary_chunks_cancellable(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    path: &std::path::Path,
    size: u64,
    mut cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<String> {
    let mut reader =
        rcw_common::transfer::FileBinaryFrameReader::new(path, size, request_id, kind)?;
    while let Some(frame) = reader.next_frame()? {
        if cancel_rx
            .as_ref()
            .map(|cancel_rx| *cancel_rx.borrow())
            .unwrap_or(false)
        {
            return Err(anyhow!("command cancelled"));
        }
        tokio::select! {
            result = sink.send(tokio_tungstenite::tungstenite::Message::Binary(frame)) => {
                result?;
            }
            changed = async {
                match cancel_rx.as_mut() {
                    Some(cancel_rx) => cancel_rx.changed().await,
                    None => std::future::pending::<Result<(), watch::error::RecvError>>().await,
                }
            } => {
                if changed.is_ok()
                    && cancel_rx
                        .as_ref()
                        .map(|cancel_rx| *cancel_rx.borrow())
                        .unwrap_or(false)
                {
                    return Err(anyhow!("command cancelled"));
                }
            }
        }
    }
    Ok(reader.finalize_sha256())
}

async fn command_screenshot(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: ScreenshotArgs = serde_json::from_value(args)?;
    if args.format != DEFAULT_SCREENSHOT_FORMAT {
        return Err(anyhow!("only png screenshots are supported"));
    }
    let bytes = platform::screenshot_png(args.display)?;
    let sha256 = sha256_bytes(&bytes);
    send_binary_chunks(sink, request_id, BinaryKind::ScreenshotChunk, &bytes).await?;
    send_complete(
        sink,
        request_id,
        session_id,
        CommandCompletePayload {
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
            size: Some(bytes.len() as u64),
            sha256: Some(sha256),
            summary: Some("screenshot captured".to_owned()),
        },
    )
    .await
}

async fn command_windows(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
) -> Result<()> {
    let windows = platform::list_windows()?;
    let data = serde_json::to_string(&windows)?;
    send_output(sink, request_id, session_id.clone(), "json", &data).await?;
    send_complete(
        sink,
        request_id,
        session_id,
        CommandCompletePayload {
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
            size: Some(windows.len() as u64),
            sha256: None,
            summary: Some("windows listed".to_owned()),
        },
    )
    .await
}

async fn command_mouse_move(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: MouseMoveArgs = serde_json::from_value(args)?;
    platform::mouse_move(args.x, args.y)?;
    complete_simple(sink, request_id, session_id, "mouse moved").await
}

async fn command_mouse_click(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: MouseClickArgs = serde_json::from_value(args)?;
    platform::mouse_click(args.x, args.y, &args.button)?;
    complete_simple(sink, request_id, session_id, "mouse clicked").await
}

async fn command_mouse_scroll(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: MouseScrollArgs = serde_json::from_value(args)?;
    platform::mouse_scroll(args.delta)?;
    complete_simple(sink, request_id, session_id, "mouse scrolled").await
}

async fn command_keyboard_type(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: KeyboardTypeArgs = serde_json::from_value(args)?;
    platform::keyboard_type(&args.text)?;
    complete_simple(sink, request_id, session_id, "text typed").await
}

async fn command_keyboard_key(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: KeyboardKeyArgs = serde_json::from_value(args)?;
    platform::keyboard_key(&args.key)?;
    complete_simple(sink, request_id, session_id, "key pressed").await
}

async fn complete_simple(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    summary: &str,
) -> Result<()> {
    send_complete(
        sink,
        request_id,
        session_id,
        CommandCompletePayload {
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
            size: None,
            sha256: None,
            summary: Some(summary.to_owned()),
        },
    )
    .await
}

async fn read_process_stream<R>(
    stream_name: &'static str,
    mut reader: R,
    tx: tokio::sync::mpsc::Sender<(String, String)>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buffer = vec![0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let data = String::from_utf8_lossy(&buffer[..read]).to_string();
                if tx.send((stream_name.to_owned(), data)).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_unsupported_errors_use_unsupported_command_code() {
        let err = anyhow!("mouse input is only supported on Windows host builds");

        assert!(matches!(
            error_code_for_command_error(&err),
            ErrorCode::UnsupportedCommand
        ));
    }

    #[test]
    fn generic_execution_errors_use_command_failed_code() {
        let err = anyhow!("process exited with status 1");

        assert!(matches!(
            error_code_for_command_error(&err),
            ErrorCode::CommandFailed
        ));
    }

    #[test]
    fn timeout_errors_use_request_timeout_code() {
        let err = anyhow!("command timed out");

        assert!(matches!(
            error_code_for_command_error(&err),
            ErrorCode::RequestTimeout
        ));
    }

    #[test]
    fn cancellation_errors_use_cancelled_code() {
        let err = anyhow!("command cancelled");

        assert!(matches!(
            error_code_for_command_error(&err),
            ErrorCode::Cancelled
        ));
    }
}
