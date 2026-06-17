use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use rcw_common::{
    audit::AuditCategory,
    ids::{new_request_id, short_machine_id},
    protocol::{
        CommandRequestPayload, ErrorCode, ErrorPayload, HostAuthResultPayload, HostHelloPayload,
        HostSessionClosePayload, HostSessionCloseResultPayload, HostSessionClosedPayload,
        HostSessionOpenedPayload, WireMessage, COMMAND_UPLOAD_BEGIN, PROTOCOL_VERSION,
        TYPE_COMMAND_CANCEL, TYPE_COMMAND_REQUEST, TYPE_ERROR, TYPE_HOST_AUTH_REQUEST,
        TYPE_HOST_AUTH_RESULT, TYPE_HOST_HELLO, TYPE_HOST_SESSION_CLOSE, TYPE_HOST_SESSION_CLOSED,
        TYPE_HOST_SESSION_CLOSE_RESULT, TYPE_HOST_SESSION_OPENED, TYPE_TUNNEL_CLOSE,
        TYPE_TUNNEL_STREAM_EOF, TYPE_TUNNEL_STREAM_OPEN, TYPE_TUNNEL_STREAM_OPEN_RESULT,
        TYPE_TUNNEL_STREAM_RESET,
    },
    totp,
    transfer::{BinaryFrame, BinaryKind},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::warn;

use crate::{
    audit::{append_host_audit, append_host_audit_record, category_for_command, HostAuditRecord},
    commands::{cancel_command_task, execute_command, prune_finished_command_tasks, CommandTasks},
    output::{close_shared_sink, send_error, send_json, shared_sink, SharedWsSink},
    platform,
    state::HostTransferCompletion,
    tunnel::{
        abort_tunnel_streams, abort_tunnel_streams_for_session, abort_tunnel_tasks,
        close_tunnel_locally, handle_tunnel_close, handle_tunnel_data, handle_tunnel_open,
        handle_tunnel_stream_eof, handle_tunnel_stream_open, handle_tunnel_stream_open_result,
        handle_tunnel_stream_reset, new_tunnel_streams, prune_finished_tunnel_tasks,
        remove_tunnels_for_session, HostTunnelTasks,
    },
    upload::{
        begin_upload, handle_binary_frame, prune_idle_uploads, remove_uploads_for_session,
        UploadState, UPLOAD_SWEEP_INTERVAL,
    },
    HostContext, HostControlRequest, HostControlResult, HostSessionControlOutcome,
    HostTaskControlOutcome, HostTaskStatus, HostTunnelControlOutcome,
};

struct HostRuntime<'a> {
    active_session: &'a mut Option<String>,
    uploads: &'a mut HashMap<String, UploadState>,
    command_tasks: &'a mut CommandTasks,
    tunnel_tasks: &'a mut HostTunnelTasks,
    tunnel_streams: &'a crate::tunnel::HostTunnelStreams,
    pending_session_closes:
        &'a mut HashMap<String, oneshot::Sender<HostControlResult<HostSessionControlOutcome>>>,
}

pub(crate) async fn run_host_connection(
    context: Arc<HostContext>,
    ws_url: String,
    mut shutdown: watch::Receiver<bool>,
    mut control_rx: Option<mpsc::Receiver<HostControlRequest>>,
) -> Result<()> {
    let (ws, _) = connect_async(ws_url)
        .await
        .context("failed to connect to rcw-server host websocket")?;
    let (sink, mut stream) = ws.split();
    let sink = shared_sink(sink);

    let hello = WireMessage::new(
        TYPE_HOST_HELLO,
        None,
        None,
        HostHelloPayload {
            protocol_version: PROTOCOL_VERSION,
            host_version: env!("CARGO_PKG_VERSION").to_owned(),
            host_id: context.host_id.clone(),
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
    )?;
    {
        let mut sink = sink.lock().await;
        send_json(&mut sink, hello).await?;
    }

    let mut active_session: Option<String> = None;
    let mut uploads: HashMap<String, UploadState> = HashMap::new();
    let mut command_tasks = CommandTasks::new();
    let mut tunnel_tasks: HostTunnelTasks = HashMap::new();
    let mut pending_session_closes: HashMap<
        String,
        oneshot::Sender<HostControlResult<HostSessionControlOutcome>>,
    > = HashMap::new();
    let tunnel_streams = new_tunnel_streams();
    let mut upload_sweep = tokio::time::interval(UPLOAD_SWEEP_INTERVAL);
    println!("Connection: connected");
    context.state.record_connected();
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
                            &sink,
                            HostRuntime {
                                active_session: &mut active_session,
                                uploads: &mut uploads,
                                command_tasks: &mut command_tasks,
                                tunnel_tasks: &mut tunnel_tasks,
                                tunnel_streams: &tunnel_streams,
                                pending_session_closes: &mut pending_session_closes,
                            },
                            message,
                        )
                        .await?;
                    }
                    Ok(Message::Binary(bytes)) => {
                        handle_host_binary(
                            &context,
                            &sink,
                            &tunnel_streams,
                            &mut uploads,
                            bytes.to_vec(),
                        )
                        .await?;
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
                prune_finished_command_tasks(&mut command_tasks);
                prune_finished_tunnel_tasks(&mut tunnel_tasks);
            }
            maybe_control = async {
                match control_rx.as_mut() {
                    Some(control_rx) => control_rx.recv().await,
                    None => std::future::pending::<Option<HostControlRequest>>().await,
                }
            } => {
                if let Some(request) = maybe_control {
                    handle_host_control_request(
                        &context,
                        &sink,
                        HostRuntime {
                            active_session: &mut active_session,
                            uploads: &mut uploads,
                            command_tasks: &mut command_tasks,
                            tunnel_tasks: &mut tunnel_tasks,
                            tunnel_streams: &tunnel_streams,
                            pending_session_closes: &mut pending_session_closes,
                        },
                        request,
                    )
                    .await?;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    abort_command_tasks(&context, &mut command_tasks);
                    abort_uploads(&context, &mut uploads);
                    abort_tunnel_tasks(&mut tunnel_tasks);
                    abort_tunnel_streams(&tunnel_streams).await;
                    close_shared_sink(&sink).await;
                    break;
                }
            }
        }
    }

    println!("Connection: disconnected");
    let disconnected_session = active_session.clone();
    context
        .state
        .record_disconnected(disconnected_session.clone());
    append_host_audit(
        &context,
        "host.disconnected",
        None,
        disconnected_session,
        None,
        Some("ok"),
    );
    for (_, respond) in pending_session_closes.drain() {
        let _ = respond.send(Err(
            "host disconnected before session close completed".to_owned()
        ));
    }
    Ok(())
}

async fn handle_server_message(
    context: &HostContext,
    sink: &SharedWsSink,
    runtime: HostRuntime<'_>,
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
            let controller_label = payload.controller_label.clone();
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
            let auth_result = WireMessage::new(
                TYPE_HOST_AUTH_RESULT,
                Some(request_id.clone()),
                None,
                result,
            )?;
            {
                let mut sink = sink.lock().await;
                send_json(&mut sink, auth_result).await?;
            }
            context
                .state
                .record_auth_request(request_id.clone(), payload.controller_label, ok);
            let mut record = HostAuditRecord::new(AuditCategory::Session, "session.auth");
            record.request_id = Some(request_id);
            record.controller_label = Some(controller_label);
            record.result = Some(if ok { "ok" } else { "failed" }.to_owned());
            append_host_audit_record(context, record);
        }
        TYPE_HOST_SESSION_OPENED => {
            let payload: HostSessionOpenedPayload = message.payload_as()?;
            *runtime.active_session = Some(payload.session_id.clone());
            println!("Session: active");
            println!("Controller: {}", payload.controller_label);
            let controller_label = payload.controller_label.clone();
            context
                .state
                .record_session_opened(payload.session_id.clone(), payload.controller_label);
            let mut record = HostAuditRecord::new(AuditCategory::Session, "session.opened");
            record.request_id = message.request_id;
            record.session_id = Some(payload.session_id);
            record.controller_label = Some(controller_label);
            record.result = Some("ok".to_owned());
            append_host_audit_record(context, record);
        }
        TYPE_HOST_SESSION_CLOSED => {
            let payload: HostSessionClosedPayload = message.payload_as()?;
            let session_id = payload.session_id.clone();
            let reason = payload.reason.clone();
            println!("Session: closed ({})", payload.reason);
            *runtime.active_session = None;
            context
                .state
                .record_session_closed(session_id.clone(), payload.reason);
            remove_uploads_for_session(runtime.uploads, &session_id);
            cancel_command_tasks_for_session(runtime.command_tasks, &session_id);
            remove_tunnels_for_session(runtime.tunnel_tasks, &session_id);
            abort_tunnel_streams_for_session(runtime.tunnel_streams, &session_id).await;
            let mut record = HostAuditRecord::new(AuditCategory::Session, "session.closed");
            record.request_id = message.request_id;
            record.session_id = Some(payload.session_id);
            record.result = Some("ok".to_owned());
            record.summary = Some(reason);
            append_host_audit_record(context, record);
        }
        TYPE_HOST_SESSION_CLOSE_RESULT => {
            let payload: HostSessionCloseResultPayload = message.payload_as()?;
            if let Some(request_id) = message.request_id {
                if let Some(respond) = runtime.pending_session_closes.remove(&request_id) {
                    let _ = respond.send(Ok(HostSessionControlOutcome {
                        closed: payload.ok,
                        session_id: Some(payload.session_id),
                    }));
                }
            }
        }
        TYPE_ERROR => {
            if let Some(request_id) = &message.request_id {
                if let Some(respond) = runtime.pending_session_closes.remove(request_id) {
                    let error = message.payload_as::<ErrorPayload>().ok();
                    let message = error
                        .map(|error| format!("{:?}: {}", error.code, error.message))
                        .unwrap_or_else(|| "host control request failed".to_owned());
                    let _ = respond.send(Err(message));
                    return Ok(());
                }
            }
        }
        TYPE_COMMAND_REQUEST => {
            let payload: CommandRequestPayload = message.payload_as()?;
            if payload.command == COMMAND_UPLOAD_BEGIN {
                if let Err(err) = begin_upload(context, runtime.uploads, message.clone(), payload) {
                    let mut sink = sink.lock().await;
                    send_error(
                        &mut sink,
                        message.request_id,
                        message.session_id,
                        ErrorCode::InvalidPath,
                        &err.to_string(),
                    )
                    .await?;
                }
            } else {
                execute_command(context, sink, runtime.command_tasks, message, payload).await?;
            }
        }
        TYPE_COMMAND_CANCEL => {
            if let Some(request_id) = &message.request_id {
                if let Some(upload) = runtime.uploads.remove(request_id) {
                    context
                        .state
                        .record_cancel_requested(request_id.clone(), upload.session_id.clone());
                    let mut record =
                        HostAuditRecord::new(AuditCategory::Transfer, "command.cancel");
                    record.request_id = Some(request_id.clone());
                    record.session_id = upload.session_id.clone();
                    record.task_id = Some(request_id.clone());
                    record.command = Some(COMMAND_UPLOAD_BEGIN.to_owned());
                    record.command_kind = Some(COMMAND_UPLOAD_BEGIN.to_owned());
                    record.result = Some("requested".to_owned());
                    append_host_audit_record(context, record);
                }
            }
            cancel_command_task(context, sink, runtime.command_tasks, message).await?;
        }
        rcw_common::protocol::TYPE_TUNNEL_OPEN => {
            handle_tunnel_open(
                context,
                sink,
                runtime.tunnel_tasks,
                runtime.tunnel_streams,
                message,
            )
            .await?;
        }
        TYPE_TUNNEL_CLOSE => {
            handle_tunnel_close(
                context,
                sink,
                runtime.tunnel_tasks,
                runtime.tunnel_streams,
                message,
            )
            .await?;
        }
        TYPE_TUNNEL_STREAM_OPEN => {
            handle_tunnel_stream_open(context, sink, runtime.tunnel_streams, message).await?;
        }
        TYPE_TUNNEL_STREAM_OPEN_RESULT => {
            handle_tunnel_stream_open_result(context, runtime.tunnel_streams, message).await?;
        }
        TYPE_TUNNEL_STREAM_EOF => {
            handle_tunnel_stream_eof(runtime.tunnel_streams, message).await?;
        }
        TYPE_TUNNEL_STREAM_RESET => {
            handle_tunnel_stream_reset(context, runtime.tunnel_streams, message).await?;
        }
        other => {
            warn!("ignored server message type {other}");
        }
    }
    Ok(())
}

async fn handle_host_control_request(
    context: &HostContext,
    sink: &SharedWsSink,
    runtime: HostRuntime<'_>,
    request: HostControlRequest,
) -> Result<()> {
    match request {
        HostControlRequest::CloseCurrentSession { reason, respond } => {
            close_current_session(context, sink, runtime, reason, respond).await
        }
        HostControlRequest::CancelTask {
            request_id,
            respond,
        } => {
            let requested = cancel_local_task(context, sink, runtime, &request_id).await?;
            let _ = respond.send(Ok(HostTaskControlOutcome {
                requested,
                request_id,
            }));
            Ok(())
        }
        HostControlRequest::CloseTunnel { tunnel_id, respond } => {
            let closed = close_tunnel_locally(
                context,
                runtime.tunnel_tasks,
                runtime.tunnel_streams,
                &tunnel_id,
            )
            .await;
            if closed {
                context.state.record_tunnel_closed(
                    tunnel_id.clone(),
                    runtime.active_session.clone(),
                    Some("host_close".to_owned()),
                );
                let mut record = HostAuditRecord::new(AuditCategory::Tunnel, "tunnel.closed");
                record.session_id = runtime.active_session.clone();
                record.task_id = Some(tunnel_id.clone());
                record.command = Some("tunnel.close".to_owned());
                record.command_kind = Some("tunnel.close".to_owned());
                record.result = Some("host_close".to_owned());
                append_host_audit_record(context, record);
            }
            let _ = respond.send(Ok(HostTunnelControlOutcome { closed, tunnel_id }));
            Ok(())
        }
    }
}

async fn close_current_session(
    context: &HostContext,
    sink: &SharedWsSink,
    runtime: HostRuntime<'_>,
    reason: String,
    respond: oneshot::Sender<HostControlResult<HostSessionControlOutcome>>,
) -> Result<()> {
    let Some(session_id) = runtime.active_session.clone() else {
        let _ = respond.send(Ok(HostSessionControlOutcome {
            closed: false,
            session_id: None,
        }));
        return Ok(());
    };
    let request_id = new_request_id();
    let mut record = HostAuditRecord::new(AuditCategory::Session, "session.close_requested");
    record.request_id = Some(request_id.clone());
    record.session_id = Some(session_id.clone());
    record.result = Some("requested".to_owned());
    record.summary = Some(reason.clone());
    append_host_audit_record(context, record);
    let close = WireMessage::new(
        TYPE_HOST_SESSION_CLOSE,
        Some(request_id.clone()),
        Some(session_id.clone()),
        HostSessionClosePayload {
            session_id: session_id.clone(),
            reason,
        },
    )?;
    {
        let mut sink = sink.lock().await;
        if let Err(err) = send_json(&mut sink, close).await {
            let _ = respond.send(Err(err.to_string()));
            return Err(err);
        }
    }
    runtime.pending_session_closes.insert(request_id, respond);
    Ok(())
}

async fn cancel_local_task(
    context: &HostContext,
    sink: &SharedWsSink,
    runtime: HostRuntime<'_>,
    request_id: &str,
) -> Result<bool> {
    let mut requested = false;
    if let Some(upload) = runtime.uploads.remove(request_id) {
        context
            .state
            .record_cancel_requested(request_id.to_owned(), upload.session_id.clone());
        let mut record = HostAuditRecord::new(AuditCategory::Transfer, "command.cancel");
        record.request_id = Some(request_id.to_owned());
        record.session_id = upload.session_id.clone();
        record.task_id = Some(request_id.to_owned());
        record.command = Some(COMMAND_UPLOAD_BEGIN.to_owned());
        record.command_kind = Some(COMMAND_UPLOAD_BEGIN.to_owned());
        record.result = Some("host_requested".to_owned());
        append_host_audit_record(context, record);
        let mut sink = sink.lock().await;
        send_error(
            &mut sink,
            Some(request_id.to_owned()),
            upload.session_id.clone(),
            ErrorCode::Cancelled,
            ErrorCode::Cancelled.message(),
        )
        .await?;
        requested = true;
    }
    if let Some(task) = runtime.command_tasks.get(request_id) {
        task.cancel();
        context
            .state
            .record_cancel_requested(request_id.to_owned(), task.session_id.clone());
        let mut record =
            HostAuditRecord::new(category_for_command(&task.command), "command.cancel");
        record.request_id = Some(request_id.to_owned());
        record.session_id = task.session_id.clone();
        record.task_id = Some(request_id.to_owned());
        record.command = Some(task.command.clone());
        record.command_kind = Some(task.command.clone());
        record.result = Some("host_requested".to_owned());
        append_host_audit_record(context, record);
        requested = true;
    }
    Ok(requested)
}

async fn handle_host_binary(
    context: &HostContext,
    sink: &SharedWsSink,
    tunnel_streams: &crate::tunnel::HostTunnelStreams,
    uploads: &mut HashMap<String, UploadState>,
    bytes: Vec<u8>,
) -> Result<()> {
    if bytes.first().copied() == Some(BinaryKind::TunnelData as u8) {
        handle_tunnel_data(tunnel_streams, bytes).await?;
        return Ok(());
    }
    let _ = BinaryFrame::decode(&bytes)?;
    let mut sink = sink.lock().await;
    handle_binary_frame(context, &mut sink, uploads, bytes).await
}

fn abort_command_tasks(context: &HostContext, command_tasks: &mut CommandTasks) {
    for (request_id, task) in command_tasks.drain() {
        context
            .state
            .record_cancel_requested(request_id, task.session_id.clone());
        task.abort();
    }
}

fn abort_uploads(context: &HostContext, uploads: &mut HashMap<String, UploadState>) {
    for (request_id, upload) in uploads.drain() {
        context
            .state
            .record_transfer_completed(HostTransferCompletion {
                request_id,
                session_id: upload.session_id.clone(),
                status: HostTaskStatus::Cancelled,
                bytes_transferred: None,
                duration_ms: None,
                result: "listener_stopped".to_owned(),
                sha256: None,
                error_message: Some("listener stopped".to_owned()),
            });
    }
}

fn cancel_command_tasks_for_session(command_tasks: &mut CommandTasks, session_id: &str) {
    let request_ids = command_tasks
        .iter()
        .filter(|(_, task)| task.session_id.as_deref() == Some(session_id))
        .map(|(request_id, _)| request_id.clone())
        .collect::<Vec<_>>();
    for request_id in request_ids {
        if let Some(task) = command_tasks.remove(&request_id) {
            task.abort();
        }
    }
}
