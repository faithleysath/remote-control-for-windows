mod platform;

use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    audit::{append_jsonl, AuditEvent},
    config,
    ids::short_machine_id,
    protocol::{
        CommandCompletePayload, CommandOutputPayload, CommandRequestPayload, DownloadArgs,
        ErrorCode, ErrorPayload, ExecArgs, HostAuthResultPayload, HostHelloPayload,
        HostSessionClosedPayload, HostSessionOpenedPayload, KeyboardKeyArgs, KeyboardTypeArgs,
        MouseClickArgs, MouseMoveArgs, MouseScrollArgs, ScreenshotArgs, UploadArgs, WireMessage,
        COMMAND_DOWNLOAD_BEGIN, COMMAND_EXEC, COMMAND_KEYBOARD_KEY, COMMAND_KEYBOARD_TYPE,
        COMMAND_MOUSE_CLICK, COMMAND_MOUSE_MOVE, COMMAND_MOUSE_SCROLL, COMMAND_SCREENSHOT,
        COMMAND_UPLOAD_BEGIN, COMMAND_WINDOWS, PROTOCOL_VERSION, TYPE_COMMAND_COMPLETE,
        TYPE_COMMAND_OUTPUT, TYPE_COMMAND_REQUEST, TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR,
        TYPE_HOST_AUTH_REQUEST, TYPE_HOST_AUTH_RESULT, TYPE_HOST_HELLO, TYPE_HOST_SESSION_CLOSED,
        TYPE_HOST_SESSION_OPENED, TYPE_UPLOAD_COMPLETE,
    },
    totp,
    transfer::{
        chunk_binary, commit_temp_output_file, create_temp_output_file, sha256_bytes,
        temp_output_path, total_sequences_for_len, BinaryFrame, BinaryKind, FileBinaryFrameReader,
        Sha256Accumulator,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::TcpStream,
    process::Command,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{error, warn};

type WsSink = futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const UPLOAD_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    totp_period_seconds: Option<u64>,
    #[arg(long)]
    audit_log: Option<PathBuf>,
}

struct HostContext {
    server_url: String,
    machine_id: String,
    totp_seed: Arc<Vec<u8>>,
    totp_period_seconds: u64,
    audit_path: PathBuf,
}

struct UploadState {
    session_id: Option<String>,
    args: UploadArgs,
    file: Option<File>,
    temp_path: PathBuf,
    next_sequence: u32,
    total_sequences: u32,
    bytes_written: u64,
    hasher: Sha256Accumulator,
    last_activity: Instant,
}

impl Drop for UploadState {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.temp_path);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().compact().init();
    let args = Args::parse();
    let server_url = config::resolve_server_url(args.server.as_deref())?;
    let ws_url = config::ws_endpoint_url(&server_url, "/ws/host")?;
    let period = config::resolve_totp_period_seconds(args.totp_period_seconds)?;
    let audit_path = args.audit_log.unwrap_or_else(platform::default_audit_path);
    let material = platform::stable_machine_material()?;
    let machine_id = short_machine_id(&material);
    let seed = Arc::new(totp::random_seed());
    let power = platform::PowerGuard::acquire();

    print_startup(
        &server_url,
        &machine_id,
        period,
        power.as_ref().map(|guard| guard.active()),
    );
    let context = Arc::new(HostContext {
        server_url: server_url.clone(),
        machine_id: machine_id.clone(),
        totp_seed: seed,
        totp_period_seconds: period,
        audit_path,
    });

    update_clipboard(&context);
    tokio::spawn(totp_refresher(context.clone()));

    loop {
        tokio::select! {
            result = run_host_connection(context.clone(), ws_url.clone()) => {
                match result {
                    Ok(()) => println!("Connection: disconnected; reconnecting"),
                    Err(err) => {
                        warn!("host connection failed: {err}");
                        println!("Connection: reconnecting ({err})");
                    }
                }
                append_host_audit(&context, "host.reconnecting", None, None, None, Some("retry"));
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            _ = tokio::signal::ctrl_c() => {
                println!("Connection: stopping");
                break;
            }
        }
    }

    drop(power);
    Ok(())
}

async fn run_host_connection(context: Arc<HostContext>, ws_url: String) -> Result<()> {
    let (ws, _) = connect_async(ws_url)
        .await
        .context("failed to connect to rcw-server host websocket")?;
    let (mut sink, mut stream) = ws.split();

    send_json(
        &mut sink,
        WireMessage::new(
            TYPE_HOST_HELLO,
            None,
            None,
            HostHelloPayload {
                protocol_version: PROTOCOL_VERSION,
                host_version: env!("CARGO_PKG_VERSION").to_owned(),
                machine_id: context.machine_id.clone(),
                totp_period_seconds: context.totp_period_seconds,
                os: std::env::consts::OS.to_owned(),
                hostname_hash: short_machine_id(
                    hostname::get()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_bytes(),
                ),
            },
        )?,
    )
    .await?;

    let mut active_session: Option<String> = None;
    let mut uploads: HashMap<String, UploadState> = HashMap::new();
    let mut upload_sweep = tokio::time::interval(UPLOAD_SWEEP_INTERVAL);
    println!("Connection: connected");
    append_host_audit(&context, "host.connected", None, None, None, Some("ok"));

    loop {
        tokio::select! {
            maybe_frame = stream.next() => {
                let Some(frame) = maybe_frame else {
                    break;
                };
                match frame {
                    Ok(Message::Text(text)) => {
                        let message: WireMessage = match serde_json::from_str(&text) {
                            Ok(message) => message,
                            Err(err) => {
                                warn!("invalid server frame: {err}");
                                continue;
                            }
                        };
                        handle_server_message(
                            &context,
                            &mut sink,
                            &mut active_session,
                            &mut uploads,
                            message,
                        )
                        .await?;
                    }
                    Ok(Message::Binary(bytes)) => {
                        handle_binary_frame(&context, &mut sink, &mut uploads, bytes).await?;
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                    Ok(Message::Frame(_)) => {}
                    Err(err) => {
                        return Err(anyhow!("host websocket error: {err}"));
                    }
                }
            }
            _ = upload_sweep.tick() => {
                prune_idle_uploads(&mut uploads);
            }
        }
    }

    println!("Connection: disconnected");
    append_host_audit(
        &context,
        "host.disconnected",
        None,
        active_session,
        None,
        Some("ok"),
    );
    Ok(())
}

async fn handle_server_message(
    context: &HostContext,
    sink: &mut WsSink,
    active_session: &mut Option<String>,
    uploads: &mut HashMap<String, UploadState>,
    message: WireMessage,
) -> Result<()> {
    match message.kind.as_str() {
        "host.hello_ack" => {
            println!("Server: hello acknowledged");
        }
        TYPE_HOST_AUTH_REQUEST => {
            let Some(request_id) = message.request_id.clone() else {
                return Ok(());
            };
            let payload: rcw_common::protocol::HostAuthRequestPayload = message.payload_as()?;
            let ok = totp::verify_code(
                &payload.totp,
                &context.totp_seed,
                context.totp_period_seconds,
                platform::unix_now(),
                totp::DEFAULT_SKEW_WINDOWS,
            )?;
            let result = HostAuthResultPayload {
                ok,
                code: (!ok).then_some(ErrorCode::InvalidTotp),
                message: (!ok).then(|| ErrorCode::InvalidTotp.message().to_owned()),
            };
            send_json(
                sink,
                WireMessage::new(
                    TYPE_HOST_AUTH_RESULT,
                    Some(request_id.clone()),
                    None,
                    result,
                )?,
            )
            .await?;
            append_host_audit(
                context,
                "session.auth",
                Some(request_id),
                None,
                None,
                Some(if ok { "ok" } else { "failed" }),
            );
        }
        TYPE_HOST_SESSION_OPENED => {
            let payload: HostSessionOpenedPayload = message.payload_as()?;
            *active_session = Some(payload.session_id.clone());
            println!("Session: active");
            println!("Controller: {}", payload.controller_label);
            append_host_audit(
                context,
                "session.opened",
                message.request_id,
                Some(payload.session_id),
                None,
                Some("ok"),
            );
        }
        TYPE_HOST_SESSION_CLOSED => {
            let payload: HostSessionClosedPayload = message.payload_as()?;
            let session_id = payload.session_id.clone();
            println!("Session: closed ({})", payload.reason);
            *active_session = None;
            remove_uploads_for_session(uploads, &session_id);
            append_host_audit(
                context,
                "session.closed",
                message.request_id,
                Some(payload.session_id),
                None,
                Some("ok"),
            );
        }
        TYPE_COMMAND_REQUEST => {
            let payload: CommandRequestPayload = message.payload_as()?;
            if payload.command == COMMAND_UPLOAD_BEGIN {
                if let Err(err) = begin_upload(context, uploads, message.clone(), payload) {
                    send_error(
                        sink,
                        message.request_id,
                        message.session_id,
                        ErrorCode::InvalidPath,
                        &err.to_string(),
                    )
                    .await?;
                }
            } else {
                execute_command(context, sink, message, payload).await?;
            }
        }
        other => {
            warn!("ignored server message type {other}");
        }
    }
    Ok(())
}

async fn execute_command(
    context: &HostContext,
    sink: &mut WsSink,
    message: WireMessage,
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
            command_exec(context, sink, &request_id, session_id.clone(), payload.args).await
        }
        COMMAND_DOWNLOAD_BEGIN => {
            command_download(sink, &request_id, session_id.clone(), payload.args).await
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

fn error_code_for_command_error(err: &anyhow::Error) -> ErrorCode {
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
    _context: &HostContext,
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
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
    let (output_tx, mut output_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(read_process_stream("stdout", stdout, output_tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(read_process_stream("stderr", stderr, output_tx));
    }

    let wait = child.wait();
    tokio::pin!(wait);
    let deadline = tokio::time::sleep(std::time::Duration::from_millis(args.timeout_ms));
    tokio::pin!(deadline);
    let status = loop {
        tokio::select! {
            Some((stream, data)) = output_rx.recv() => {
                send_output(sink, request_id, session_id.clone(), &stream, &data).await?;
            }
            result = &mut wait => {
                break result?;
            }
            _ = &mut deadline => {
                if let Some(pid) = pid {
                    let _ = platform::kill_process_tree(pid);
                }
                send_error(
                    sink,
                    Some(request_id.to_owned()),
                    session_id,
                    ErrorCode::RequestTimeout,
                    ErrorCode::RequestTimeout.message(),
                )
                .await?;
                return Err(anyhow!("command timed out"));
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

fn begin_upload(
    context: &HostContext,
    uploads: &mut HashMap<String, UploadState>,
    message: WireMessage,
    payload: CommandRequestPayload,
) -> Result<()> {
    let request_id = message
        .request_id
        .clone()
        .ok_or_else(|| anyhow!("upload command missing request_id"))?;
    let args: UploadArgs = serde_json::from_value(payload.args)?;
    if args.remote_path.trim().is_empty() {
        return Err(anyhow!("empty upload path"));
    }
    let total_sequences = total_sequences_for_len(args.size)?;
    let remote_path = PathBuf::from(&args.remote_path);
    let temp_path = temp_output_path(&remote_path, &request_id);
    let file = create_temp_output_file(&remote_path, &temp_path, args.overwrite)?;
    println!(
        "[{}] upload waiting for chunks request={}",
        rcw_common::audit::now_rfc3339(),
        request_id
    );
    append_host_audit(
        context,
        "command.started",
        Some(request_id.clone()),
        message.session_id.clone(),
        Some(payload.command),
        Some("started"),
    );
    uploads.insert(
        request_id,
        UploadState {
            session_id: message.session_id,
            args,
            file: Some(file),
            temp_path,
            next_sequence: 0,
            total_sequences,
            bytes_written: 0,
            hasher: Sha256Accumulator::new(),
            last_activity: Instant::now(),
        },
    );
    Ok(())
}

async fn command_download(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: DownloadArgs = serde_json::from_value(args)?;
    let path = PathBuf::from(&args.remote_path);
    let size = fs::metadata(&path)?.len();
    let sha256 =
        send_file_binary_chunks(sink, request_id, BinaryKind::DownloadChunk, &path, size).await?;
    send_complete_kind(
        sink,
        TYPE_DOWNLOAD_COMPLETE,
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

async fn command_screenshot(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    args: serde_json::Value,
) -> Result<()> {
    let args: ScreenshotArgs = serde_json::from_value(args)?;
    if args.format != "png" {
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

async fn handle_binary_frame(
    context: &HostContext,
    sink: &mut WsSink,
    uploads: &mut HashMap<String, UploadState>,
    bytes: Vec<u8>,
) -> Result<()> {
    let frame = BinaryFrame::decode(&bytes)?;
    if frame.kind != BinaryKind::UploadChunk {
        send_error(
            sink,
            Some(frame.request_id),
            None,
            ErrorCode::UnsupportedCommand,
            "host only accepts upload binary chunks from controller",
        )
        .await?;
        return Ok(());
    }

    let request_id = frame.request_id.clone();
    if !uploads.contains_key(&request_id) {
        send_error(
            sink,
            Some(request_id),
            None,
            ErrorCode::SessionExpired,
            "upload chunk has no active upload request",
        )
        .await?;
        return Ok(());
    }

    let action = {
        let state = uploads
            .get_mut(&request_id)
            .ok_or_else(|| anyhow!("upload state disappeared"))?;
        if frame.total_sequences != state.total_sequences {
            Some((
                state.session_id.clone(),
                ErrorCode::InternalError,
                format!(
                    "upload chunk total sequence mismatch: expected {}, got {}",
                    state.total_sequences, frame.total_sequences
                ),
            ))
        } else if frame.sequence != state.next_sequence {
            Some((
                state.session_id.clone(),
                ErrorCode::InternalError,
                format!(
                    "upload chunk sequence mismatch: expected {}, got {}",
                    state.next_sequence, frame.sequence
                ),
            ))
        } else {
            let file = state
                .file
                .as_mut()
                .ok_or_else(|| anyhow!("upload file handle is closed"))?;
            file.write_all(&frame.payload)?;
            state.hasher.update(&frame.payload);
            state.bytes_written += frame.payload.len() as u64;
            state.next_sequence += 1;
            state.last_activity = Instant::now();
            None
        }
    };

    if let Some((session_id, code, message)) = action {
        uploads.remove(&request_id);
        send_error(sink, Some(request_id), session_id, code, &message).await?;
        return Ok(());
    }

    let complete = uploads
        .get(&request_id)
        .map(|state| state.next_sequence == state.total_sequences)
        .unwrap_or(false);
    if complete {
        let state = uploads
            .remove(&request_id)
            .ok_or_else(|| anyhow!("upload disappeared during finalization"))?;
        finalize_upload(context, sink, &request_id, state).await?;
    }
    Ok(())
}

async fn finalize_upload(
    context: &HostContext,
    sink: &mut WsSink,
    request_id: &str,
    mut state: UploadState,
) -> Result<()> {
    if let Some(mut file) = state.file.take() {
        file.flush()?;
    }
    if state.bytes_written != state.args.size {
        send_error(
            sink,
            Some(request_id.to_owned()),
            state.session_id.clone(),
            ErrorCode::ChecksumMismatch,
            "upload size mismatch",
        )
        .await?;
        return Err(anyhow!("upload size mismatch"));
    }
    let actual = std::mem::replace(&mut state.hasher, Sha256Accumulator::new()).finalize();
    if actual != state.args.sha256 {
        send_error(
            sink,
            Some(request_id.to_owned()),
            state.session_id.clone(),
            ErrorCode::ChecksumMismatch,
            ErrorCode::ChecksumMismatch.message(),
        )
        .await?;
        return Err(anyhow!("upload checksum mismatch"));
    }
    commit_temp_output_file(
        &state.temp_path,
        Path::new(&state.args.remote_path),
        state.args.overwrite,
    )?;
    send_complete_kind(
        sink,
        TYPE_UPLOAD_COMPLETE,
        request_id,
        state.session_id.clone(),
        CommandCompletePayload {
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
            size: Some(state.bytes_written),
            sha256: Some(actual),
            summary: Some(format!("wrote {}", state.args.remote_path)),
        },
    )
    .await?;
    append_host_audit(
        context,
        "command.complete",
        Some(request_id.to_owned()),
        state.session_id.clone(),
        Some(COMMAND_UPLOAD_BEGIN.to_owned()),
        Some("ok"),
    );
    Ok(())
}

async fn read_process_stream<R>(
    stream_name: &'static str,
    mut reader: R,
    tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buffer = vec![0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let data = String::from_utf8_lossy(&buffer[..read]).to_string();
                let _ = tx.send((stream_name.to_owned(), data));
            }
            Err(_) => break,
        }
    }
}

async fn send_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    bytes: &[u8],
) -> Result<()> {
    for frame in chunk_binary(request_id, kind, bytes)? {
        sink.send(Message::Binary(frame)).await?;
    }
    Ok(())
}

fn remove_uploads_for_session(uploads: &mut HashMap<String, UploadState>, session_id: &str) {
    let before = uploads.len();
    uploads.retain(|_, state| state.session_id.as_deref() != Some(session_id));
    let removed = before.saturating_sub(uploads.len());
    if removed > 0 {
        warn!(session_id = %session_id, removed, "removed pending uploads for closed session");
    }
}

fn prune_idle_uploads(uploads: &mut HashMap<String, UploadState>) {
    let now = Instant::now();
    uploads.retain(|request_id, state| {
        let keep = now.duration_since(state.last_activity) <= UPLOAD_IDLE_TIMEOUT;
        if !keep {
            warn!(request_id = %request_id, "removed idle pending upload");
        }
        keep
    });
}

async fn send_file_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    path: &Path,
    size: u64,
) -> Result<String> {
    let mut reader = FileBinaryFrameReader::new(path, size, request_id, kind)?;
    while let Some(frame) = reader.next_frame()? {
        sink.send(Message::Binary(frame)).await?;
    }
    Ok(reader.finalize_sha256())
}

async fn send_output(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    stream: &str,
    data: &str,
) -> Result<()> {
    send_json(
        sink,
        WireMessage::new(
            TYPE_COMMAND_OUTPUT,
            Some(request_id.to_owned()),
            session_id,
            CommandOutputPayload {
                stream: stream.to_owned(),
                data: data.to_owned(),
            },
        )?,
    )
    .await
}

async fn send_complete(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    payload: CommandCompletePayload,
) -> Result<()> {
    send_complete_kind(sink, TYPE_COMMAND_COMPLETE, request_id, session_id, payload).await
}

async fn send_complete_kind(
    sink: &mut WsSink,
    kind: &str,
    request_id: &str,
    session_id: Option<String>,
    payload: CommandCompletePayload,
) -> Result<()> {
    send_json(
        sink,
        WireMessage::new(kind, Some(request_id.to_owned()), session_id, payload)?,
    )
    .await
}

async fn send_error(
    sink: &mut WsSink,
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) -> Result<()> {
    send_json(
        sink,
        WireMessage::new(
            TYPE_ERROR,
            request_id,
            session_id,
            ErrorPayload {
                code,
                message: message.to_owned(),
            },
        )?,
    )
    .await
}

async fn send_json(sink: &mut WsSink, message: WireMessage) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(&message)?))
        .await?;
    Ok(())
}

async fn totp_refresher(context: Arc<HostContext>) {
    loop {
        platform::sleep_until_next_totp_tick(context.totp_period_seconds).await;
        update_clipboard(&context);
    }
}

fn update_clipboard(context: &HostContext) {
    let code = totp::current_code(
        &context.totp_seed,
        context.totp_period_seconds,
        platform::unix_now(),
    )
    .unwrap_or_else(|_| "000000".to_owned());
    let text = format!(
        "远程协助连接信息\n服务器：{}\n机器 ID：{}\n验证码：{}\n验证码有效期：{} 秒\n",
        context.server_url, context.machine_id, code, context.totp_period_seconds
    );
    match platform::copy_connection_info(&text) {
        Ok(()) => println!("Clipboard: connection info copied"),
        Err(err) => println!("Clipboard: copy failed ({err}); copy ID/TOTP manually"),
    }
    println!("Machine ID: {}", context.machine_id);
    println!("Current TOTP: {code}");
}

fn print_startup(
    server_url: &str,
    machine_id: &str,
    period: u64,
    power_active: Result<bool, &anyhow::Error>,
) {
    println!("Remote Control for Windows Host");
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("Server: {server_url}");
    if platform::is_elevated() {
        println!("Privilege: ADMINISTRATOR / elevated");
    } else {
        println!("Privilege: standard user");
    }
    println!("Machine ID: {machine_id}");
    println!("TOTP period: {period}s");
    match power_active {
        Ok(true) => println!("Power: sleep/display timeout suppressed while host is running"),
        Ok(false) => println!("Power: no platform power request active"),
        Err(err) => println!("Power: warning: {err}"),
    }
    println!("Keep this window open while support is active.");
    println!("Close this window to stop remote control.");
}

fn append_host_audit(
    context: &HostContext,
    event: &str,
    request_id: Option<String>,
    session_id: Option<String>,
    command: Option<String>,
    result: Option<&str>,
) {
    let mut audit = AuditEvent::new("host", event);
    audit.machine_id = Some(context.machine_id.clone());
    audit.request_id = request_id;
    audit.session_id = session_id;
    audit.command = command;
    audit.result = result.map(str::to_owned);
    if let Err(err) = append_jsonl(&context.audit_path, &audit) {
        warn!("failed to write host audit log: {err}");
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
}
