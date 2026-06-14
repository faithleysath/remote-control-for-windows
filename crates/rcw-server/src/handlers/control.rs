use crate::{
    audit,
    state::PendingOpen,
    ws::{
        log_websocket_read_error, outbound_channel, send_binary, send_error, send_text,
        spawn_writer, Tx,
    },
    AppState,
};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::StreamExt;
use rcw_common::{
    audit::AuditEvent,
    ids::token_label,
    protocol::{
        CommandCancelPayload, CommandCancelResultPayload, CommandRequestPayload,
        ControlOpenPayload, ErrorCode, HostAuthRequestPayload, HostSessionClosedPayload,
        SessionClosePayload, SessionCloseResultPayload, SessionStatusPayload,
        SessionStatusResultPayload, WireMessage, PROTOCOL_VERSION, TYPE_COMMAND_CANCEL,
        TYPE_COMMAND_CANCEL_RESULT, TYPE_COMMAND_REQUEST, TYPE_CONTROL_OPEN,
        TYPE_HOST_AUTH_REQUEST, TYPE_HOST_SESSION_CLOSED, TYPE_SESSION_CLOSE,
        TYPE_SESSION_CLOSE_RESULT, TYPE_SESSION_STATUS, TYPE_SESSION_STATUS_RESULT,
    },
    transfer::BinaryFrame,
};

pub(crate) async fn control_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_control_socket(socket, state))
}

async fn handle_control_socket(socket: WebSocket, state: AppState) {
    let (sender, mut receiver) = socket.split();
    let (tx, rx) = outbound_channel();
    let writer = spawn_writer(sender, rx, "control");

    while let Some(frame) = receiver.next().await {
        match frame {
            Ok(Message::Text(text)) => match serde_json::from_str::<WireMessage>(&text) {
                Ok(message) => handle_control_message(&state, &tx, message).await,
                Err(err) => send_error(
                    &tx,
                    None,
                    None,
                    ErrorCode::InternalError,
                    &format!("invalid json frame: {err}"),
                ),
            },
            Ok(Message::Binary(bytes)) => handle_control_binary(&state, &tx, bytes).await,
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                log_websocket_read_error("control", err);
                break;
            }
        }
    }

    writer.abort();
    state.inner.clear_request_routes_for_controller(&tx).await;
}

async fn handle_control_message(state: &AppState, tx: &Tx, message: WireMessage) {
    match message.kind.as_str() {
        TYPE_CONTROL_OPEN => handle_control_open(state, tx, message).await,
        TYPE_SESSION_STATUS => handle_session_status(state, tx, message).await,
        TYPE_SESSION_CLOSE => handle_session_close(state, tx, message).await,
        TYPE_COMMAND_REQUEST => handle_command_request(state, tx, message).await,
        TYPE_COMMAND_CANCEL => handle_command_cancel(state, tx, message).await,
        other => send_error(
            tx,
            message.request_id,
            message.session_id,
            ErrorCode::UnsupportedCommand,
            &format!("unsupported message type: {other}"),
        ),
    }
}

async fn handle_command_cancel(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let Some(request_id_value) = request_id.clone() else {
        send_error(
            tx,
            None,
            message.session_id.clone(),
            ErrorCode::InternalError,
            "command.cancel requires request_id",
        );
        return;
    };
    let payload = match message.payload_as::<CommandCancelPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                message.session_id.clone(),
                ErrorCode::InternalError,
                &format!("invalid command.cancel payload: {err}"),
            );
            return;
        }
    };
    let Some(session_id) = state.inner.request_session_id(&request_id_value).await else {
        send_error(
            tx,
            request_id,
            message.session_id,
            ErrorCode::SessionExpired,
            "command.cancel has no active request route",
        );
        return;
    };
    let Some(session) = state
        .inner
        .session_if_valid(&session_id, &payload.session_token)
        .await
    else {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };
    let machine_id = session.machine_id.clone();
    let session_id = session.session_id.clone();
    let Some(host_tx) = state.inner.host_tx(&machine_id).await else {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };

    if !send_text(&host_tx, message) {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    }
    state.inner.touch_session(&session_id).await;
    let result = WireMessage::new(
        TYPE_COMMAND_CANCEL_RESULT,
        request_id,
        Some(session_id),
        CommandCancelResultPayload { ok: true },
    )
    .expect("command cancel result serializes");
    send_text(tx, result);
}

async fn handle_control_binary(state: &AppState, tx: &Tx, bytes: Vec<u8>) {
    let frame = match BinaryFrame::decode(&bytes) {
        Ok(frame) => frame,
        Err(err) => {
            send_error(
                tx,
                None,
                None,
                ErrorCode::InternalError,
                &format!("invalid binary frame: {err}"),
            );
            return;
        }
    };
    let request_id = frame.request_id.clone();
    let session_id = state.inner.request_session_id(&request_id).await;
    let Some(session_id) = session_id else {
        send_error(
            tx,
            Some(request_id),
            None,
            ErrorCode::SessionExpired,
            "binary frame has no active request route",
        );
        return;
    };
    let Some((machine_id, session_id)) = state.inner.command_route(&session_id).await else {
        send_error(
            tx,
            Some(request_id),
            Some(session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };
    state.inner.touch_session(&session_id).await;
    let Some(host_tx) = state.inner.host_tx(&machine_id).await else {
        send_error(
            tx,
            Some(request_id),
            Some(session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };
    send_binary(&host_tx, bytes);
}

async fn handle_control_open(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let open = match message.payload_as::<ControlOpenPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                None,
                ErrorCode::InternalError,
                &format!("invalid control.open payload: {err}"),
            );
            return;
        }
    };

    if open.protocol_version != PROTOCOL_VERSION {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::InternalError,
            "unsupported protocol version",
        );
        return;
    }
    if !state.inner.allow_auth_attempt(&open.machine_id).await {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::PermissionDenied,
            "authentication rate limit exceeded",
        );
        return;
    }
    if open.control_token != *state.control_token {
        send_error(
            tx,
            request_id.clone(),
            None,
            ErrorCode::InvalidToken,
            ErrorCode::InvalidToken.message(),
        );
        audit(
            state,
            AuditEvent {
                machine_id: Some(open.machine_id),
                request_id,
                result: Some("failed".to_owned()),
                summary: Some("invalid control token".to_owned()),
                ..AuditEvent::new("server", "session.auth")
            },
        );
        return;
    }

    let host = state.inner.host(&open.machine_id).await;
    let Some(host) = host else {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::HostNotFound,
            ErrorCode::HostNotFound.message(),
        );
        return;
    };

    if host.totp_period_seconds != open.totp_period_seconds {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::InvalidTotpPeriod,
            ErrorCode::InvalidTotpPeriod.message(),
        );
        return;
    }

    if !open.force_reconnect && state.inner.host_has_active_session(&open.machine_id).await {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::HostBusy,
            ErrorCode::HostBusy.message(),
        );
        return;
    }

    let Some(request_id) = message.request_id.clone() else {
        send_error(
            tx,
            None,
            None,
            ErrorCode::InternalError,
            "control.open requires request_id",
        );
        return;
    };
    let controller_label = token_label(&open.control_token);
    state
        .inner
        .insert_pending_open(
            request_id.clone(),
            PendingOpen {
                machine_id: open.machine_id.clone(),
                controller_tx: tx.clone(),
                controller_label: controller_label.clone(),
                force_reconnect: open.force_reconnect,
                created_at: std::time::Instant::now(),
            },
        )
        .await;

    let auth = WireMessage::new(
        TYPE_HOST_AUTH_REQUEST,
        Some(request_id.clone()),
        None,
        HostAuthRequestPayload {
            totp: open.totp,
            controller_label,
        },
    )
    .expect("auth request serializes");

    if !send_text(&host.tx, auth) {
        state.inner.remove_pending_open(&request_id).await;
        send_error(
            tx,
            Some(request_id),
            None,
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
    }
}

async fn handle_session_status(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let Some(session_id) = message.session_id.clone() else {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::SessionExpired,
            "missing session_id",
        );
        return;
    };
    let payload = match message.payload_as::<SessionStatusPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid session.status payload: {err}"),
            );
            return;
        }
    };

    let status = state
        .inner
        .session_if_valid(&session_id, &payload.session_token)
        .await;

    let Some(session) = status else {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };
    let host_online = state.inner.host_tx(&session.machine_id).await.is_some();
    let result = WireMessage::new(
        TYPE_SESSION_STATUS_RESULT,
        request_id,
        Some(session.session_id.clone()),
        SessionStatusResultPayload {
            ok: true,
            machine_id: session.machine_id,
            host_online,
            session_active: host_online,
        },
    )
    .expect("status result serializes");
    send_text(tx, result);
}

async fn handle_session_close(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let Some(session_id) = message.session_id.clone() else {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::SessionExpired,
            "missing session_id",
        );
        return;
    };
    let payload = match message.payload_as::<SessionClosePayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid session.close payload: {err}"),
            );
            return;
        }
    };

    let session = state
        .inner
        .remove_session_if_valid(&session_id, &payload.session_token)
        .await;

    let Some(session) = session else {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };

    if let Some(host_tx) = state.inner.host_tx(&session.machine_id).await {
        let closed = WireMessage::new(
            TYPE_HOST_SESSION_CLOSED,
            request_id.clone(),
            Some(session.session_id.clone()),
            HostSessionClosedPayload {
                session_id: session.session_id.clone(),
                reason: "controller_close".to_owned(),
            },
        )
        .expect("session closed serializes");
        send_text(&host_tx, closed);
    }

    let response = WireMessage::new(
        TYPE_SESSION_CLOSE_RESULT,
        request_id.clone(),
        Some(session.session_id.clone()),
        SessionCloseResultPayload {
            ok: true,
            session_id: session.session_id.clone(),
        },
    )
    .expect("close result serializes");
    send_text(tx, response);

    audit(
        state,
        AuditEvent {
            machine_id: Some(session.machine_id),
            session_id: Some(session.session_id),
            request_id,
            result: Some("ok".to_owned()),
            ..AuditEvent::new("server", "session.closed")
        },
    );
}

async fn handle_command_request(state: &AppState, tx: &Tx, mut message: WireMessage) {
    let request_id = message.request_id.clone();
    let Some(request_id_value) = request_id.clone() else {
        send_error(
            tx,
            None,
            message.session_id.clone(),
            ErrorCode::InternalError,
            "command.request requires request_id",
        );
        return;
    };
    let Some(session_id) = message.session_id.clone() else {
        send_error(
            tx,
            request_id,
            None,
            ErrorCode::SessionExpired,
            "missing session_id",
        );
        return;
    };
    let payload = match message.payload_as::<CommandRequestPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid command.request payload: {err}"),
            );
            return;
        }
    };

    let session = state
        .inner
        .bind_controller_to_session(&session_id, &payload.session_token, tx.clone())
        .await;

    let Some(session) = session else {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };

    let Some(host_tx) = state.inner.host_tx(&session.machine_id).await else {
        send_error(
            tx,
            request_id,
            Some(session.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };

    message.session_id = Some(session.session_id.clone());
    state
        .inner
        .track_request_route(
            request_id_value.clone(),
            session.session_id.clone(),
            tx.clone(),
        )
        .await;
    if !send_text(&host_tx, message) {
        state.inner.clear_request_route(&request_id_value).await;
        send_error(
            tx,
            request_id,
            Some(session.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    }

    audit(
        state,
        AuditEvent {
            machine_id: Some(session.machine_id),
            session_id: Some(session.session_id),
            request_id,
            command: Some(payload.command),
            audit_label: payload.audit_label,
            result: Some("relayed".to_owned()),
            ..AuditEvent::new("server", "command.relay")
        },
    );
}
