use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use rcw_common::{
    ids::short_machine_id,
    protocol::{
        CommandRequestPayload, ErrorCode, HostAuthResultPayload, HostHelloPayload,
        HostSessionClosedPayload, HostSessionOpenedPayload, WireMessage, COMMAND_UPLOAD_BEGIN,
        PROTOCOL_VERSION, TYPE_COMMAND_CANCEL, TYPE_COMMAND_REQUEST, TYPE_HOST_AUTH_REQUEST,
        TYPE_HOST_AUTH_RESULT, TYPE_HOST_HELLO, TYPE_HOST_SESSION_CLOSED, TYPE_HOST_SESSION_OPENED,
        TYPE_TUNNEL_CLOSE, TYPE_TUNNEL_STREAM_EOF, TYPE_TUNNEL_STREAM_OPEN,
        TYPE_TUNNEL_STREAM_OPEN_RESULT, TYPE_TUNNEL_STREAM_RESET,
    },
    totp,
    transfer::{BinaryFrame, BinaryKind},
};
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::warn;

use crate::{
    audit::append_host_audit,
    commands::{cancel_command_task, execute_command, prune_finished_command_tasks, CommandTasks},
    output::{close_shared_sink, send_error, send_json, shared_sink, SharedWsSink},
    platform,
    tunnel::{
        abort_tunnel_streams, abort_tunnel_streams_for_session, abort_tunnel_tasks,
        handle_tunnel_close, handle_tunnel_data, handle_tunnel_open, handle_tunnel_stream_eof,
        handle_tunnel_stream_open, handle_tunnel_stream_open_result, handle_tunnel_stream_reset,
        new_tunnel_streams, prune_finished_tunnel_tasks, remove_tunnels_for_session,
        HostTunnelTasks,
    },
    upload::{
        begin_upload, handle_binary_frame, prune_idle_uploads, remove_uploads_for_session,
        UploadState, UPLOAD_SWEEP_INTERVAL,
    },
    HostContext,
};

struct HostRuntime<'a> {
    active_session: &'a mut Option<String>,
    uploads: &'a mut HashMap<String, UploadState>,
    command_tasks: &'a mut CommandTasks,
    tunnel_tasks: &'a mut HostTunnelTasks,
    tunnel_streams: &'a crate::tunnel::HostTunnelStreams,
}

pub(crate) async fn run_host_connection(
    context: Arc<HostContext>,
    ws_url: String,
    mut shutdown: watch::Receiver<bool>,
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
    let tunnel_streams = new_tunnel_streams();
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
                            &sink,
                            HostRuntime {
                                active_session: &mut active_session,
                                uploads: &mut uploads,
                                command_tasks: &mut command_tasks,
                                tunnel_tasks: &mut tunnel_tasks,
                                tunnel_streams: &tunnel_streams,
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
                            bytes,
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
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    abort_command_tasks(&mut command_tasks);
                    abort_tunnel_tasks(&mut tunnel_tasks);
                    abort_tunnel_streams(&tunnel_streams).await;
                    close_shared_sink(&sink).await;
                    break;
                }
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
            *runtime.active_session = Some(payload.session_id.clone());
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
            *runtime.active_session = None;
            remove_uploads_for_session(runtime.uploads, &session_id);
            cancel_command_tasks_for_session(runtime.command_tasks, &session_id);
            remove_tunnels_for_session(runtime.tunnel_tasks, &session_id);
            abort_tunnel_streams_for_session(runtime.tunnel_streams, &session_id).await;
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
                runtime.uploads.remove(request_id);
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
            handle_tunnel_stream_reset(runtime.tunnel_streams, message).await?;
        }
        other => {
            warn!("ignored server message type {other}");
        }
    }
    Ok(())
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

fn abort_command_tasks(command_tasks: &mut CommandTasks) {
    for (_, task) in command_tasks.drain() {
        task.abort();
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
