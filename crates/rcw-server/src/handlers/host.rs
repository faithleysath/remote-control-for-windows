use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    audit::AuditEvent,
    ids::{new_request_id, new_session_id, new_session_token},
    protocol::{
        CommandCompletePayload, CommandOutputPayload, ControlOpenResultPayload, ErrorCode,
        ErrorPayload, HostAuthResultPayload, HostHelloAckPayload, HostHelloPayload,
        HostSessionClosedPayload, HostSessionOpenedPayload, WireMessage, PROTOCOL_VERSION,
        TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT, TYPE_CONTROL_OPEN_RESULT,
        TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR, TYPE_HOST_AUTH_RESULT, TYPE_HOST_HELLO,
        TYPE_HOST_HELLO_ACK, TYPE_HOST_SESSION_CLOSED, TYPE_HOST_SESSION_OPENED,
        TYPE_UPLOAD_COMPLETE,
    },
    transfer::BinaryFrame,
};
use tracing::{debug, info, warn};

use crate::{
    audit,
    state::HostConn,
    ws::{
        log_websocket_read_error, make_error, outbound_channel, send_binary, send_error, send_text,
        spawn_writer, HEARTBEAT_INTERVAL_MS,
    },
    AppState,
};

pub(crate) async fn host_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_host_socket(socket, state))
}

async fn handle_host_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let Some(Ok(Message::Text(first))) = receiver.next().await else {
        return;
    };

    let hello_msg = match serde_json::from_str::<WireMessage>(&first) {
        Ok(message) => message,
        Err(err) => {
            warn!("invalid host hello frame: {err}");
            return;
        }
    };

    if hello_msg.kind != TYPE_HOST_HELLO {
        let error = make_error(
            hello_msg.request_id.clone(),
            None,
            ErrorCode::InternalError,
            "first host frame must be host.hello",
        );
        let _ = sender
            .send(Message::Text(
                serde_json::to_string(&error).unwrap_or_default(),
            ))
            .await;
        return;
    }

    let hello = match hello_msg.payload_as::<HostHelloPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            warn!("invalid host hello payload: {err}");
            return;
        }
    };

    if hello.protocol_version != PROTOCOL_VERSION {
        let error = make_error(
            hello_msg.request_id.clone(),
            None,
            ErrorCode::InternalError,
            "unsupported protocol version",
        );
        let _ = sender
            .send(Message::Text(
                serde_json::to_string(&error).unwrap_or_default(),
            ))
            .await;
        return;
    }

    if !state.inner.allow_host_registration(&hello.host_id).await {
        let error = make_error(
            hello_msg.request_id.clone(),
            None,
            ErrorCode::PermissionDenied,
            "host registration rate limit exceeded",
        );
        let _ = sender
            .send(Message::Text(
                serde_json::to_string(&error).unwrap_or_default(),
            ))
            .await;
        return;
    }

    let (tx, rx) = outbound_channel();
    let writer = spawn_writer(sender, rx, "host");
    let connection_id = new_request_id();

    let replaced = state
        .inner
        .register_host(
            hello.host_id.clone(),
            hello.machine_id.clone(),
            connection_id.clone(),
            tx.clone(),
            hello.totp_period_seconds,
        )
        .await;
    if let Some(replaced) = replaced {
        close_replaced_host_connection(&state, replaced).await;
    }

    audit(
        &state,
        AuditEvent {
            machine_id: Some(hello.machine_id.clone()),
            host_id: Some(hello.host_id.clone()),
            summary: Some(format!(
                "host connected host_id={} connection_id={} version={} os={}",
                hello.host_id, connection_id, hello.host_version, hello.os
            )),
            ..AuditEvent::new("server", "host.connected")
        },
    );

    let ack = WireMessage::new(
        TYPE_HOST_HELLO_ACK,
        hello_msg.request_id.clone(),
        None,
        HostHelloAckPayload {
            server_time: rcw_common::audit::now_rfc3339(),
            heartbeat_interval_ms: HEARTBEAT_INTERVAL_MS,
        },
    )
    .expect("host hello ack serializes");
    send_text(&tx, ack);

    info!(
        "host {} ({}) connected connection={}",
        hello.machine_id, hello.host_id, connection_id
    );

    while let Some(frame) = receiver.next().await {
        match frame {
            Ok(Message::Text(text)) => match serde_json::from_str::<WireMessage>(&text) {
                Ok(message) => {
                    handle_host_message(&state, &hello.host_id, &connection_id, message).await
                }
                Err(err) => warn!("invalid host message from {}: {err}", hello.machine_id),
            },
            Ok(Message::Binary(bytes)) => {
                relay_host_binary_response(&state, &hello.host_id, &connection_id, bytes).await;
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                log_websocket_read_error(&format!("host {}", hello.machine_id), err);
                break;
            }
        }
    }

    writer.abort();
    unregister_host(&state, &hello.host_id, &connection_id, &hello.machine_id).await;
}

async fn close_replaced_host_connection(state: &AppState, replaced: HostConn) {
    for (request_id, pending) in state
        .inner
        .remove_pending_open_for_host(&replaced.host_id, Some(&replaced.connection_id))
        .await
    {
        send_error(
            &pending.controller_tx,
            Some(request_id),
            None,
            ErrorCode::HostDisconnected,
            "host connection replaced before authentication completed",
        );
    }

    let removed_sessions = state
        .inner
        .remove_sessions_for_host(
            &replaced.host_id,
            Some(&replaced.connection_id),
            rcw_common::protocol::ErrorPayload {
                code: ErrorCode::HostDisconnected,
                message: "host connection replaced".to_owned(),
            },
        )
        .await;
    for session in removed_sessions {
        if let Some(tx) = session.controller_tx {
            send_error(
                &tx,
                None,
                Some(session.session_id.clone()),
                ErrorCode::HostDisconnected,
                "host connection replaced",
            );
        }
        audit(
            state,
            AuditEvent {
                machine_id: Some(session.machine_id),
                host_id: Some(replaced.host_id.clone()),
                session_id: Some(session.session_id),
                result: Some("closed".to_owned()),
                summary: Some("host connection replaced".to_owned()),
                ..AuditEvent::new("server", "session.closed")
            },
        );
    }
}

async fn handle_host_message(
    state: &AppState,
    host_id: &str,
    connection_id: &str,
    message: WireMessage,
) {
    match message.kind.as_str() {
        TYPE_HOST_AUTH_RESULT => {
            handle_host_auth_result(state, host_id, connection_id, message).await
        }
        TYPE_COMMAND_OUTPUT
        | TYPE_COMMAND_COMPLETE
        | TYPE_UPLOAD_COMPLETE
        | TYPE_DOWNLOAD_COMPLETE
        | TYPE_ERROR => relay_host_response(state, host_id, connection_id, message).await,
        other => debug!("ignored host message type {other} from {host_id}"),
    }
}

async fn handle_host_auth_result(
    state: &AppState,
    host_id: &str,
    connection_id: &str,
    message: WireMessage,
) {
    let Some(request_id) = message.request_id.clone() else {
        return;
    };
    let result = match message.payload_as::<HostAuthResultPayload>() {
        Ok(result) => result,
        Err(err) => {
            warn!("invalid host auth result: {err}");
            return;
        }
    };
    let pending = state.inner.take_pending_open(&request_id).await;

    let Some(pending) = pending else {
        warn!("host auth result for unknown request {request_id}");
        return;
    };

    if pending.host_id != host_id || pending.connection_id != connection_id {
        warn!("host auth result machine mismatch for request {request_id}");
        send_error(
            &pending.controller_tx,
            Some(request_id),
            None,
            ErrorCode::InternalError,
            "auth result machine mismatch",
        );
        return;
    }

    if !result.ok {
        let code = result.code.unwrap_or(ErrorCode::InvalidTotp);
        send_error(
            &pending.controller_tx,
            Some(request_id.clone()),
            None,
            code,
            result.message.as_deref().unwrap_or(code.message()),
        );
        audit(
            state,
            AuditEvent {
                machine_id: Some(pending.machine_id),
                host_id: Some(host_id.to_owned()),
                request_id: Some(request_id),
                result: Some("failed".to_owned()),
                summary: Some(code.message().to_owned()),
                ..AuditEvent::new("server", "session.auth")
            },
        );
        return;
    }

    if pending.force_reconnect {
        close_existing_sessions_for_force_reconnect(state, host_id, connection_id, &request_id)
            .await;
    }

    let session_id = new_session_id();
    let session_token = new_session_token();
    state
        .inner
        .create_session(
            session_id.clone(),
            session_token.clone(),
            host_id.to_owned(),
            pending.machine_id.clone(),
            connection_id.to_owned(),
            pending.controller_tx.clone(),
        )
        .await;

    let result = WireMessage::new(
        TYPE_CONTROL_OPEN_RESULT,
        Some(request_id.clone()),
        Some(session_id.clone()),
        ControlOpenResultPayload {
            ok: true,
            session_id: session_id.clone(),
            session_token,
            host_id: host_id.to_owned(),
            machine_id: pending.machine_id.clone(),
        },
    )
    .expect("open result serializes");
    send_text(&pending.controller_tx, result);

    if let Some(host_tx) = state.inner.host_tx(host_id, connection_id).await {
        let opened = WireMessage::new(
            TYPE_HOST_SESSION_OPENED,
            Some(request_id.clone()),
            Some(session_id.clone()),
            HostSessionOpenedPayload {
                session_id: session_id.clone(),
                controller_label: pending.controller_label.clone(),
            },
        )
        .expect("session opened serializes");
        send_text(&host_tx, opened);
    }

    audit(
        state,
        AuditEvent {
            machine_id: Some(pending.machine_id),
            host_id: Some(host_id.to_owned()),
            session_id: Some(session_id),
            request_id: Some(request_id),
            result: Some("ok".to_owned()),
            ..AuditEvent::new("server", "session.created")
        },
    );
}

async fn close_existing_sessions_for_force_reconnect(
    state: &AppState,
    host_id: &str,
    connection_id: &str,
    request_id: &str,
) {
    let removed_sessions = state
        .inner
        .remove_sessions_for_host(
            host_id,
            Some(connection_id),
            rcw_common::protocol::ErrorPayload {
                code: ErrorCode::Cancelled,
                message: "session replaced by force reconnect".to_owned(),
            },
        )
        .await;
    if removed_sessions.is_empty() {
        return;
    }

    let host_tx = state.inner.host_tx(host_id, connection_id).await;
    for session in removed_sessions {
        if let Some(tx) = &session.controller_tx {
            send_error(
                tx,
                Some(request_id.to_owned()),
                Some(session.session_id.clone()),
                ErrorCode::SessionExpired,
                "session replaced by force reconnect",
            );
        }
        if let Some(tx) = &host_tx {
            let closed = WireMessage::new(
                TYPE_HOST_SESSION_CLOSED,
                Some(request_id.to_owned()),
                Some(session.session_id.clone()),
                HostSessionClosedPayload {
                    session_id: session.session_id.clone(),
                    reason: "force_reconnect".to_owned(),
                },
            )
            .expect("session closed serializes");
            send_text(tx, closed);
        }
        audit(
            state,
            AuditEvent {
                machine_id: Some(session.machine_id),
                host_id: Some(host_id.to_owned()),
                session_id: Some(session.session_id),
                request_id: Some(request_id.to_owned()),
                result: Some("closed".to_owned()),
                summary: Some("force reconnect".to_owned()),
                ..AuditEvent::new("server", "session.closed")
            },
        );
    }
}

async fn relay_host_response(
    state: &AppState,
    host_id: &str,
    connection_id: &str,
    message: WireMessage,
) {
    let request_id = message.request_id.clone();
    let is_terminal = matches!(
        message.kind.as_str(),
        TYPE_COMMAND_COMPLETE | TYPE_UPLOAD_COMPLETE | TYPE_DOWNLOAD_COMPLETE | TYPE_ERROR
    );
    let Some(session_id) = message.session_id.clone() else {
        warn!("host response without session_id from {host_id}");
        return;
    };

    let route = if let Some(request_id) = &request_id {
        state
            .inner
            .request_route_for_host(request_id, host_id, connection_id, Some(&session_id))
            .await
    } else {
        None
    };
    let is_detached = route.as_ref().map(|route| route.detached).unwrap_or(false);
    if is_detached {
        if let Some(request_id) = &request_id {
            record_detached_exec_response(state, request_id, &message).await;
        }
    }
    let mut controller_tx = route
        .as_ref()
        .filter(|route| !route.detached)
        .map(|route| route.controller_tx.clone());
    if controller_tx.is_none() && !is_detached {
        controller_tx = state
            .inner
            .session_controller_for_machine(&session_id, host_id, connection_id)
            .await;
    }
    debug!(
        kind = %message.kind,
        request_id = ?request_id,
        session_id = %session_id,
        host_id = %host_id,
        connection_id = %connection_id,
        has_controller = controller_tx.is_some(),
        terminal = is_terminal,
        "relaying host text response"
    );
    if let Some(tx) = controller_tx {
        send_text(&tx, message);
    };
    if is_terminal {
        if let Some(request_id) = request_id {
            state.inner.clear_request_route(&request_id).await;
        }
    }
}

async fn record_detached_exec_response(state: &AppState, request_id: &str, message: &WireMessage) {
    match message.kind.as_str() {
        TYPE_COMMAND_OUTPUT => {
            if let Ok(output) = message.payload_as::<CommandOutputPayload>() {
                state
                    .inner
                    .append_exec_job_output(request_id, &output.stream, &output.data)
                    .await;
            }
        }
        TYPE_COMMAND_COMPLETE => {
            if let Ok(complete) = message.payload_as::<CommandCompletePayload>() {
                state.inner.finish_exec_job(request_id, complete).await;
            }
        }
        TYPE_ERROR => {
            if let Ok(error) = message.payload_as::<ErrorPayload>() {
                state.inner.fail_exec_job(request_id, error).await;
            }
        }
        _ => {}
    }
}

async fn relay_host_binary_response(
    state: &AppState,
    host_id: &str,
    connection_id: &str,
    bytes: Vec<u8>,
) {
    let frame = match BinaryFrame::decode(&bytes) {
        Ok(frame) => frame,
        Err(err) => {
            warn!("invalid binary frame from host {host_id}: {err}");
            return;
        }
    };
    let route = state
        .inner
        .request_route_for_host(&frame.request_id, host_id, connection_id, None)
        .await;
    let Some(route) = route else {
        warn!(
            "host binary frame for unknown or mismatched request {} from {}",
            frame.request_id, host_id
        );
        return;
    };
    let mut controller_tx = (!route.detached).then(|| route.controller_tx.clone());
    if controller_tx.is_none() {
        controller_tx = state
            .inner
            .session_controller_for_machine(&route.session_id, host_id, connection_id)
            .await;
    }
    debug!(
        request_id = %frame.request_id,
        session_id = %route.session_id,
        host_id = %host_id,
        connection_id = %connection_id,
        bytes = bytes.len(),
        has_controller = controller_tx.is_some(),
        "relaying host binary response"
    );
    if let Some(tx) = controller_tx {
        send_binary(&tx, bytes);
    };
}

async fn unregister_host(state: &AppState, host_id: &str, connection_id: &str, machine_id: &str) {
    let removed_sessions = state.inner.unregister_host(host_id, connection_id).await;

    for session in removed_sessions {
        if let Some(tx) = session.controller_tx {
            send_error(
                &tx,
                None,
                Some(session.session_id.clone()),
                ErrorCode::HostDisconnected,
                ErrorCode::HostDisconnected.message(),
            );
        }
        audit(
            state,
            AuditEvent {
                machine_id: Some(session.machine_id),
                host_id: Some(host_id.to_owned()),
                session_id: Some(session.session_id),
                result: Some("closed".to_owned()),
                summary: Some("host disconnected".to_owned()),
                ..AuditEvent::new("server", "session.closed")
            },
        );
    }

    audit(
        state,
        AuditEvent {
            machine_id: Some(machine_id.to_owned()),
            host_id: Some(host_id.to_owned()),
            result: Some("ok".to_owned()),
            summary: Some(format!("host_id={host_id} connection_id={connection_id}")),
            ..AuditEvent::new("server", "host.disconnected")
        },
    );
    info!("host {machine_id} disconnected");
}
