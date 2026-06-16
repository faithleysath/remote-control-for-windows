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
    audit::{
        append_host_audit_record, command_audit_details, path_summary, sanitize_audit_text,
        CommandAuditDetails, HostAuditRecord,
    },
    output::{
        send_binary_chunks, send_complete, send_error, send_output, send_shared_complete,
        send_shared_complete_kind, send_shared_error, send_shared_output, SharedWsSink, WsSink,
    },
    platform,
    state::{
        task_status_for_result, HostCommandCompletion, HostTransferCompletion, HostTransferStart,
    },
    HostContext, HostTaskStatus, HostTransferDirection,
};

const PROCESS_OUTPUT_QUEUE_CAPACITY: usize = 128;

pub(crate) type CommandTasks = HashMap<String, CommandTask>;

pub(crate) struct CommandTask {
    pub(crate) session_id: Option<String>,
    pub(crate) command: String,
    cancel_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

struct DownloadSend<'a> {
    context: &'a HostContext,
    request_id: &'a str,
    session_id: Option<String>,
    kind: BinaryKind,
    path: &'a std::path::Path,
    size: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
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
        context
            .state
            .record_cancel_requested(request_id.clone(), message.session_id.clone());
        let mut record = HostAuditRecord::new(
            crate::audit::category_for_command(&task.command),
            "command.cancel",
        );
        record.request_id = Some(request_id);
        record.session_id = message.session_id;
        record.task_id = record.request_id.clone();
        record.command = Some(task.command.clone());
        record.command_kind = Some(task.command.clone());
        record.result = Some("requested".to_owned());
        append_host_audit_record(context, record);
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
    let command_name = payload.command.clone();
    let audit_label = payload.audit_label.clone();
    let audit_details = command_audit_details(&command_name, &payload.args);
    let started = Instant::now();
    let started_at = rcw_common::audit::now_rfc3339();

    println!(
        "[{}] {} started request={}",
        started_at, command_name, request_id
    );
    record_task_started(
        context,
        &request_id,
        session_id.clone(),
        &command_name,
        &audit_details,
        None,
    );
    append_command_started_audit(
        context,
        &request_id,
        session_id.clone(),
        &command_name,
        audit_label.clone(),
        &audit_details,
        started_at,
    );

    let (result, complete) = match payload.command.as_str() {
        COMMAND_EXEC => {
            let result = command_exec(
                context,
                sink,
                &request_id,
                session_id.clone(),
                payload.args,
                None,
            )
            .await;
            let complete = result.as_ref().ok().cloned();
            (result.map(|_| ()), complete)
        }
        COMMAND_DOWNLOAD_BEGIN => {
            let result = command_download(
                context,
                sink,
                &request_id,
                session_id.clone(),
                payload.args,
                None,
            )
            .await;
            let complete = result.as_ref().ok().cloned();
            (result.map(|_| ()), complete)
        }
        COMMAND_SCREENSHOT => (
            command_screenshot(sink, &request_id, session_id.clone(), payload.args).await,
            None,
        ),
        COMMAND_WINDOWS => (
            command_windows(sink, &request_id, session_id.clone()).await,
            None,
        ),
        COMMAND_MOUSE_MOVE => (
            command_mouse_move(sink, &request_id, session_id.clone(), payload.args).await,
            None,
        ),
        COMMAND_MOUSE_CLICK => (
            command_mouse_click(sink, &request_id, session_id.clone(), payload.args).await,
            None,
        ),
        COMMAND_MOUSE_SCROLL => (
            command_mouse_scroll(sink, &request_id, session_id.clone(), payload.args).await,
            None,
        ),
        COMMAND_KEYBOARD_TYPE => (
            command_keyboard_type(sink, &request_id, session_id.clone(), payload.args).await,
            None,
        ),
        COMMAND_KEYBOARD_KEY => (
            command_keyboard_key(sink, &request_id, session_id.clone(), payload.args).await,
            None,
        ),
        _ => (Err(anyhow!("unsupported command")), None),
    };

    let ok = result.is_ok();
    let status = task_status_for_result(ok, result.as_ref().err());
    let result_label = task_result_label(status);
    let duration_ms = started.elapsed().as_millis() as u64;
    let finished_at = rcw_common::audit::now_rfc3339();
    println!(
        "[{}] {} {} request={}",
        finished_at,
        command_name,
        if ok { "ok" } else { "failed" },
        request_id
    );
    record_task_completed(
        context,
        TaskCompletion {
            request_id: &request_id,
            session_id: session_id.clone(),
            command: &command_name,
            status,
            duration_ms,
            result: result_label,
            bytes_transferred: None,
            complete,
            error: result.as_ref().err(),
        },
    );
    append_command_completed_audit(
        context,
        CommandCompleteAudit {
            request_id: &request_id,
            session_id: session_id.clone(),
            command: &command_name,
            audit_label,
            details: &audit_details,
            result: if ok { "ok" } else { "failed" },
            duration_ms,
            finished_at,
            error: result.as_ref().err(),
        },
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
    let command = payload.command.clone();
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
            command,
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
    let command_name = payload.command.clone();
    let audit_label = payload.audit_label.clone();
    let audit_details = command_audit_details(&command_name, &payload.args);
    let started_at = rcw_common::audit::now_rfc3339();

    println!(
        "[{}] {} started request={}",
        started_at, command_name, request_id
    );
    record_task_started(
        &context,
        &request_id,
        session_id.clone(),
        &command_name,
        &audit_details,
        None,
    );
    append_command_started_audit(
        &context,
        &request_id,
        session_id.clone(),
        &command_name,
        audit_label.clone(),
        &audit_details,
        started_at,
    );

    let (result, complete) = match payload.command.as_str() {
        COMMAND_EXEC => {
            let result = command_exec_shared(
                &context,
                &sink,
                &request_id,
                session_id.clone(),
                payload.args,
                Some(cancel_rx),
            )
            .await;
            let complete = result.as_ref().ok().cloned();
            (result.map(|_| ()), complete)
        }
        COMMAND_DOWNLOAD_BEGIN => {
            let result = command_download_shared(
                &context,
                &sink,
                &request_id,
                session_id.clone(),
                payload.args,
                Some(cancel_rx),
            )
            .await;
            let complete = result.as_ref().ok().cloned();
            (result.map(|_| ()), complete)
        }
        _ => (Err(anyhow!("unsupported async command")), None),
    };

    let ok = result.is_ok();
    let status = task_status_for_result(ok, result.as_ref().err());
    let result_label = task_result_label(status);
    let duration_ms = started.elapsed().as_millis() as u64;
    let finished_at = rcw_common::audit::now_rfc3339();
    println!(
        "[{}] {} {} request={}",
        finished_at,
        command_name,
        if ok { "ok" } else { "failed" },
        request_id
    );
    record_task_completed(
        &context,
        TaskCompletion {
            request_id: &request_id,
            session_id: session_id.clone(),
            command: &command_name,
            status,
            duration_ms,
            result: result_label,
            bytes_transferred: None,
            complete,
            error: result.as_ref().err(),
        },
    );
    append_command_completed_audit(
        &context,
        CommandCompleteAudit {
            request_id: &request_id,
            session_id: session_id.clone(),
            command: &command_name,
            audit_label,
            details: &audit_details,
            result: if ok { "ok" } else { "failed" },
            duration_ms,
            finished_at,
            error: result.as_ref().err(),
        },
    );

    if let Err(err) = &result {
        error!(
            "command failed after {} ms: {err}",
            started.elapsed().as_millis()
        );
        let code = error_code_for_command_error(err);
        let _ = send_shared_error(
            &sink,
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

fn record_task_started(
    context: &HostContext,
    request_id: &str,
    session_id: Option<String>,
    command: &str,
    details: &CommandAuditDetails,
    size: Option<u64>,
) {
    if command == COMMAND_DOWNLOAD_BEGIN {
        context.state.record_transfer_started(HostTransferStart {
            request_id: request_id.to_owned(),
            session_id,
            direction: HostTransferDirection::Download,
            size,
            remote_path_summary: details.path_summary.clone(),
            local_path_summary: None,
            sha256: details.sha256.clone(),
        });
    } else {
        context.state.record_command_started(
            request_id.to_owned(),
            session_id,
            command.to_owned(),
            details.args_summary.clone(),
            details.path_summary.clone(),
        );
    }
}

struct TaskCompletion<'a> {
    request_id: &'a str,
    session_id: Option<String>,
    command: &'a str,
    status: HostTaskStatus,
    duration_ms: u64,
    result: String,
    bytes_transferred: Option<u64>,
    complete: Option<CommandCompletePayload>,
    error: Option<&'a anyhow::Error>,
}

struct CommandCompleteAudit<'a> {
    request_id: &'a str,
    session_id: Option<String>,
    command: &'a str,
    audit_label: Option<String>,
    details: &'a CommandAuditDetails,
    result: &'a str,
    duration_ms: u64,
    finished_at: String,
    error: Option<&'a anyhow::Error>,
}

fn record_task_completed(context: &HostContext, completion: TaskCompletion<'_>) {
    if completion.command == COMMAND_DOWNLOAD_BEGIN {
        let complete = completion.complete;
        context
            .state
            .record_transfer_completed(HostTransferCompletion {
                request_id: completion.request_id.to_owned(),
                session_id: completion.session_id,
                status: completion.status,
                bytes_transferred: completion
                    .bytes_transferred
                    .or_else(|| complete.as_ref().and_then(|complete| complete.size)),
                duration_ms: Some(completion.duration_ms),
                result: completion.result,
                sha256: complete
                    .as_ref()
                    .and_then(|complete| complete.sha256.clone()),
                error_message: completion
                    .error
                    .map(|error| sanitize_audit_text(&error.to_string())),
            });
    } else {
        context
            .state
            .record_command_completed(HostCommandCompletion {
                request_id: completion.request_id.to_owned(),
                session_id: completion.session_id,
                command: completion.command.to_owned(),
                status: completion.status,
                duration_ms: completion.duration_ms,
                result: completion.result,
                exit_code: completion.complete.and_then(|complete| complete.exit_code),
                error_message: completion
                    .error
                    .map(|error| sanitize_audit_text(&error.to_string())),
            });
    }
}

fn task_result_label(status: HostTaskStatus) -> String {
    match status {
        HostTaskStatus::Running => "running",
        HostTaskStatus::Completed => "completed",
        HostTaskStatus::Failed => "failed",
        HostTaskStatus::Cancelled => "cancelled",
        HostTaskStatus::Timeout => "timeout",
    }
    .to_owned()
}

fn append_command_started_audit(
    context: &HostContext,
    request_id: &str,
    session_id: Option<String>,
    command: &str,
    audit_label: Option<String>,
    details: &CommandAuditDetails,
    started_at: String,
) {
    let mut record = HostAuditRecord::new(details.category, "command.started");
    record.request_id = Some(request_id.to_owned());
    record.session_id = session_id;
    record.task_id = Some(request_id.to_owned());
    record.command = Some(command.to_owned());
    record.command_kind = Some(command.to_owned());
    record.audit_label = audit_label;
    record.result = Some("started".to_owned());
    record.started_at = Some(started_at);
    record.args_summary = details.args_summary.clone();
    record.path_summary = details.path_summary.clone();
    record.bytes = details.bytes;
    record.size = details.size;
    record.sha256 = details.sha256.clone();
    append_host_audit_record(context, record);
}

fn append_command_completed_audit(context: &HostContext, audit: CommandCompleteAudit<'_>) {
    let mut record = HostAuditRecord::new(audit.details.category, "command.complete");
    record.request_id = Some(audit.request_id.to_owned());
    record.session_id = audit.session_id;
    record.task_id = Some(audit.request_id.to_owned());
    record.command = Some(audit.command.to_owned());
    record.command_kind = Some(audit.command.to_owned());
    record.audit_label = audit.audit_label;
    record.result = Some(audit.result.to_owned());
    record.duration_ms = Some(audit.duration_ms);
    record.finished_at = Some(audit.finished_at);
    record.args_summary = audit.details.args_summary.clone();
    record.path_summary = audit.details.path_summary.clone();
    record.bytes = audit.details.bytes;
    record.size = audit.details.size;
    record.sha256 = audit.details.sha256.clone();
    if let Some(error) = audit.error {
        record.error_code = Some(format!("{:?}", error_code_for_command_error(error)));
        record.error_message = Some(error.to_string());
    }
    append_host_audit_record(context, record);
}

async fn command_exec(
    context: &HostContext,
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<CommandCompletePayload> {
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
                context
                    .state
                    .record_command_output(request_id, &stream, data.len(), false);
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
        context
            .state
            .record_command_output(request_id, &stream, data.len(), false);
        send_output(sink, request_id, session_id.clone(), &stream, &data).await?;
    }

    let complete = CommandCompletePayload {
        ok: status.success(),
        exit_code: status.code(),
        duration_ms: started.elapsed().as_millis() as u64,
        size: None,
        sha256: None,
        summary: Some(format!("program={}", args.program)),
    };
    send_complete(sink, request_id, session_id, complete.clone()).await?;
    Ok(complete)
}

async fn command_exec_shared(
    context: &HostContext,
    sink: &SharedWsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<CommandCompletePayload> {
    let args: ExecArgs = serde_json::from_value(args)?;
    let started = Instant::now();
    let mut command = Command::new(&args.program);
    command.args(&args.argv);
    if let Some(cwd) = args.cwd.clone() {
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
                context
                    .state
                    .record_command_output(request_id, &stream, data.len(), false);
                send_shared_output(sink, request_id, session_id.clone(), &stream, &data).await?;
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
        context
            .state
            .record_command_output(request_id, &stream, data.len(), false);
        send_shared_output(sink, request_id, session_id.clone(), &stream, &data).await?;
    }

    let complete = CommandCompletePayload {
        ok: status.success(),
        exit_code: status.code(),
        duration_ms: started.elapsed().as_millis() as u64,
        size: None,
        sha256: None,
        summary: Some(format!("program={}", args.program)),
    };
    send_shared_complete(sink, request_id, session_id, complete.clone()).await?;
    Ok(complete)
}

async fn command_download(
    context: &HostContext,
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<CommandCompletePayload> {
    let args: DownloadArgs = serde_json::from_value(args)?;
    let path = PathBuf::from(&args.remote_path);
    let size = fs::metadata(&path)?.len();
    context
        .state
        .record_transfer_size(request_id.to_owned(), session_id.clone(), size);
    let sha256 = send_file_binary_chunks_cancellable(
        sink,
        DownloadSend {
            context,
            request_id,
            session_id: session_id.clone(),
            kind: BinaryKind::DownloadChunk,
            path: &path,
            size,
            cancel_rx,
        },
    )
    .await?;
    let complete = CommandCompletePayload {
        ok: true,
        exit_code: Some(0),
        duration_ms: 0,
        size: Some(size),
        sha256: Some(sha256),
        summary: Some(format!("read {}", path_summary(&args.remote_path))),
    };
    crate::output::send_complete_kind(
        sink,
        rcw_common::protocol::TYPE_DOWNLOAD_COMPLETE,
        request_id,
        session_id,
        complete.clone(),
    )
    .await?;
    Ok(complete)
}

async fn command_download_shared(
    context: &HostContext,
    sink: &SharedWsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<CommandCompletePayload> {
    let args: DownloadArgs = serde_json::from_value(args)?;
    let path = PathBuf::from(&args.remote_path);
    let size = fs::metadata(&path)?.len();
    context
        .state
        .record_transfer_size(request_id.to_owned(), session_id.clone(), size);
    let sha256 = send_file_binary_chunks_shared_cancellable(
        sink,
        DownloadSend {
            context,
            request_id,
            session_id: session_id.clone(),
            kind: BinaryKind::DownloadChunk,
            path: &path,
            size,
            cancel_rx,
        },
    )
    .await?;
    let complete = CommandCompletePayload {
        ok: true,
        exit_code: Some(0),
        duration_ms: 0,
        size: Some(size),
        sha256: Some(sha256),
        summary: Some(format!("read {}", path_summary(&args.remote_path))),
    };
    send_shared_complete_kind(
        sink,
        rcw_common::protocol::TYPE_DOWNLOAD_COMPLETE,
        request_id,
        session_id,
        complete.clone(),
    )
    .await?;
    Ok(complete)
}

async fn send_file_binary_chunks_cancellable(
    sink: &mut WsSink,
    mut send: DownloadSend<'_>,
) -> Result<String> {
    let mut reader = rcw_common::transfer::FileBinaryFrameReader::new(
        send.path,
        send.size,
        send.request_id,
        send.kind,
    )?;
    while let Some(frame) = reader.next_frame()? {
        let bytes_transferred = reader.bytes_transferred();
        if send
            .cancel_rx
            .as_ref()
            .map(|cancel_rx| *cancel_rx.borrow())
            .unwrap_or(false)
        {
            return Err(anyhow!("command cancelled"));
        }
        tokio::select! {
            result = sink.send(tokio_tungstenite::tungstenite::Message::Binary(frame)) => {
                result?;
                send.context.state.record_transfer_progress(
                    send.request_id.to_owned(),
                    send.session_id.clone(),
                    bytes_transferred,
                );
            }
            changed = async {
                match send.cancel_rx.as_mut() {
                    Some(cancel_rx) => cancel_rx.changed().await,
                    None => std::future::pending::<Result<(), watch::error::RecvError>>().await,
                }
            } => {
                if changed.is_ok()
                    && send
                        .cancel_rx
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

async fn send_file_binary_chunks_shared_cancellable(
    sink: &SharedWsSink,
    mut send: DownloadSend<'_>,
) -> Result<String> {
    let mut reader = rcw_common::transfer::FileBinaryFrameReader::new(
        send.path,
        send.size,
        send.request_id,
        send.kind,
    )?;
    while let Some(frame) = reader.next_frame()? {
        let bytes_transferred = reader.bytes_transferred();
        if send
            .cancel_rx
            .as_ref()
            .map(|cancel_rx| *cancel_rx.borrow())
            .unwrap_or(false)
        {
            return Err(anyhow!("command cancelled"));
        }
        tokio::select! {
            result = async {
                let mut sink = sink.lock().await;
                sink.send(tokio_tungstenite::tungstenite::Message::Binary(frame)).await
            } => {
                result?;
                send.context.state.record_transfer_progress(
                    send.request_id.to_owned(),
                    send.session_id.clone(),
                    bytes_transferred,
                );
            }
            changed = async {
                match send.cancel_rx.as_mut() {
                    Some(cancel_rx) => cancel_rx.changed().await,
                    None => std::future::pending::<Result<(), watch::error::RecvError>>().await,
                }
            } => {
                if changed.is_ok()
                    && send
                        .cancel_rx
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
