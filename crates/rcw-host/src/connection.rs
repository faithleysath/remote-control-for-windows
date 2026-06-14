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
    },
    totp,
};
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::warn;

use crate::{
    audit::append_host_audit,
    commands::{cancel_command_task, execute_command, prune_finished_command_tasks, CommandTasks},
    output::{close_shared_sink, send_error, send_json, shared_sink, SharedWsSink},
    platform,
    upload::{
        begin_upload, handle_binary_frame, prune_idle_uploads, remove_uploads_for_session,
        UploadState, UPLOAD_SWEEP_INTERVAL,
    },
    HostContext,
};

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
                            &mut active_session,
                            &mut uploads,
                            &mut command_tasks,
                            message,
                        )
                        .await?;
                    }
                    Ok(Message::Binary(bytes)) => {
                        let mut sink = sink.lock().await;
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
                prune_finished_command_tasks(&mut command_tasks);
            }
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    abort_command_tasks(&mut command_tasks);
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
    active_session: &mut Option<String>,
    uploads: &mut HashMap<String, UploadState>,
    command_tasks: &mut CommandTasks,
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
            cancel_command_tasks_for_session(command_tasks, &session_id);
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
                execute_command(context, sink, command_tasks, message, payload).await?;
            }
        }
        TYPE_COMMAND_CANCEL => {
            if let Some(request_id) = &message.request_id {
                uploads.remove(request_id);
            }
            cancel_command_task(context, sink, command_tasks, message).await?;
        }
        other => {
            warn!("ignored server message type {other}");
        }
    }
    Ok(())
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
