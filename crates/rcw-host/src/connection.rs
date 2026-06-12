use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use rcw_common::{
    ids::short_machine_id,
    protocol::{
        CommandRequestPayload, ErrorCode, HostAuthResultPayload, HostHelloPayload,
        HostSessionClosedPayload, HostSessionOpenedPayload, WireMessage, COMMAND_UPLOAD_BEGIN,
        PROTOCOL_VERSION, TYPE_COMMAND_REQUEST, TYPE_HOST_AUTH_REQUEST, TYPE_HOST_AUTH_RESULT,
        TYPE_HOST_HELLO, TYPE_HOST_SESSION_CLOSED, TYPE_HOST_SESSION_OPENED,
    },
    totp,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::warn;

use crate::{
    audit::append_host_audit,
    commands::execute_command,
    output::{send_error, send_json, WsSink},
    platform,
    upload::{
        begin_upload, handle_binary_frame, prune_idle_uploads, remove_uploads_for_session,
        UploadState, UPLOAD_SWEEP_INTERVAL,
    },
    HostContext,
};

pub(crate) async fn run_host_connection(context: Arc<HostContext>, ws_url: String) -> Result<()> {
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
