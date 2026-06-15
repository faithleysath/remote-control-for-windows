use crate::{
    audit,
    state::CreateTunnel,
    state::{CreateExecJob, HostLookup, PendingOpen},
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
        CommandStatusPayload, ControlOpenPayload, ErrorCode, HostAuthRequestPayload,
        HostSessionClosedPayload, SessionClosePayload, SessionCloseResultPayload,
        SessionStatusPayload, SessionStatusResultPayload, TunnelClosePayload,
        TunnelCloseResultPayload, TunnelOpenPayload, TunnelStatusPayload,
        TunnelStatusResultPayload, TunnelStreamControlPayload, TunnelStreamOpenPayload,
        WireMessage, PROTOCOL_VERSION, TYPE_COMMAND_CANCEL, TYPE_COMMAND_CANCEL_RESULT,
        TYPE_COMMAND_REQUEST, TYPE_COMMAND_START, TYPE_COMMAND_START_RESULT, TYPE_COMMAND_STATUS,
        TYPE_COMMAND_STATUS_RESULT, TYPE_CONTROL_OPEN, TYPE_HOST_AUTH_REQUEST,
        TYPE_HOST_SESSION_CLOSED, TYPE_SESSION_CLOSE, TYPE_SESSION_CLOSE_RESULT,
        TYPE_SESSION_STATUS, TYPE_SESSION_STATUS_RESULT, TYPE_TUNNEL_CLOSE,
        TYPE_TUNNEL_CLOSE_RESULT, TYPE_TUNNEL_OPEN, TYPE_TUNNEL_STATUS, TYPE_TUNNEL_STATUS_RESULT,
        TYPE_TUNNEL_STREAM_EOF, TYPE_TUNNEL_STREAM_OPEN, TYPE_TUNNEL_STREAM_RESET,
    },
    transfer::{BinaryFrame, BinaryKind, TunnelDataFrame},
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
        TYPE_COMMAND_START => handle_command_start(state, tx, message).await,
        TYPE_COMMAND_STATUS => handle_command_status(state, tx, message).await,
        TYPE_COMMAND_CANCEL => handle_command_cancel(state, tx, message).await,
        TYPE_TUNNEL_OPEN => handle_tunnel_open(state, tx, message).await,
        TYPE_TUNNEL_STATUS => handle_tunnel_status(state, tx, message).await,
        TYPE_TUNNEL_CLOSE => handle_tunnel_close(state, tx, message).await,
        TYPE_TUNNEL_STREAM_OPEN => handle_tunnel_stream_open(state, tx, message).await,
        rcw_common::protocol::TYPE_TUNNEL_STREAM_OPEN_RESULT => {
            handle_tunnel_stream_open_result(state, tx, message).await
        }
        TYPE_TUNNEL_STREAM_EOF => handle_tunnel_stream_eof(state, tx, message).await,
        TYPE_TUNNEL_STREAM_RESET => handle_tunnel_stream_reset(state, tx, message).await,
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
    let route = match state.inner.request_session_id(&request_id_value).await {
        Some(session_id) => Some((session_id, false)),
        None => state
            .inner
            .exec_job_route_if_valid(&request_id_value, &payload.session_token)
            .await
            .map(|(_, _, session_id)| (session_id, true)),
    };
    let Some((session_id, _from_job)) = route else {
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
    let session_id = session.session_id.clone();
    let Some(host_tx) = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
    else {
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

async fn handle_command_status(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let payload = match message.payload_as::<CommandStatusPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                message.session_id.clone(),
                ErrorCode::InternalError,
                &format!("invalid command.status payload: {err}"),
            );
            return;
        }
    };
    let Some(snapshot) = state
        .inner
        .exec_job_if_valid(&payload.task_id, &payload.session_token)
        .await
    else {
        send_error(
            tx,
            request_id,
            message.session_id,
            ErrorCode::SessionExpired,
            "command.status has no active exec job",
        );
        return;
    };
    let result = WireMessage::new(
        TYPE_COMMAND_STATUS_RESULT,
        request_id,
        message.session_id,
        snapshot,
    )
    .expect("command status result serializes");
    send_text(tx, result);
}

async fn handle_control_binary(state: &AppState, tx: &Tx, bytes: Vec<u8>) {
    if bytes.first().copied() == Some(BinaryKind::TunnelData as u8) {
        handle_control_tunnel_data(state, tx, bytes).await;
        return;
    }
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
    let route = state.inner.request_route(&request_id).await;
    let Some(route) = route else {
        send_error(
            tx,
            Some(request_id),
            None,
            ErrorCode::SessionExpired,
            "binary frame has no active request route",
        );
        return;
    };
    let Some(session) = state.inner.command_route(&route.session_id).await else {
        send_error(
            tx,
            Some(request_id),
            Some(route.session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };
    if session.host_id != route.host_id || session.connection_id != route.connection_id {
        send_error(
            tx,
            Some(request_id),
            Some(session.session_id),
            ErrorCode::SessionExpired,
            "binary frame route does not match session host",
        );
        return;
    }
    state.inner.touch_session(&session.session_id).await;
    let Some(host_tx) = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
    else {
        send_error(
            tx,
            Some(request_id),
            Some(session.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };
    send_binary(&host_tx, bytes);
}

async fn handle_control_tunnel_data(state: &AppState, tx: &Tx, bytes: Vec<u8>) {
    let frame = match TunnelDataFrame::decode(&bytes) {
        Ok(frame) => frame,
        Err(err) => {
            send_error(
                tx,
                None,
                None,
                ErrorCode::InternalError,
                &format!("invalid tunnel data frame: {err}"),
            );
            return;
        }
    };
    let Some(route) = state
        .inner
        .record_tunnel_bytes(
            &frame.tunnel_id,
            &frame.stream_id,
            rcw_common::protocol::TunnelEndpointSide::Controller,
            frame.payload.len(),
        )
        .await
    else {
        send_error(
            tx,
            None,
            None,
            ErrorCode::SessionExpired,
            "tunnel data has no active stream route",
        );
        return;
    };
    let Some(host_tx) = state
        .inner
        .host_tx(&route.host_id, &route.connection_id)
        .await
    else {
        send_error(
            tx,
            None,
            Some(route.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };
    send_binary(&host_tx, bytes);
}

async fn handle_tunnel_open(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let Some(request_id_value) = request_id.clone() else {
        send_error(
            tx,
            None,
            message.session_id.clone(),
            ErrorCode::InternalError,
            "tunnel.open requires request_id",
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
    let payload = match message.payload_as::<TunnelOpenPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid tunnel.open payload: {err}"),
            );
            return;
        }
    };
    if let Err(err) = validate_tunnel_open_payload(&payload) {
        send_error(tx, request_id, Some(session_id), err.code, &err.message);
        return;
    }
    let Some(session) = state
        .inner
        .bind_controller_to_session(&session_id, &payload.session_token, tx.clone())
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
    let Some(host_tx) = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
    else {
        send_error(
            tx,
            request_id,
            Some(session.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };
    let tunnel_id = rcw_common::ids::new_tunnel_id();
    let created = state
        .inner
        .create_tunnel(CreateTunnel {
            tunnel_id: tunnel_id.clone(),
            session: session.clone(),
            direction: payload.direction,
            listen_addr: payload.listen_addr.clone(),
            listen_port: payload.listen_port,
            target_host: payload.target_host.clone(),
            target_port: payload.target_port,
            idle_timeout_ms: payload.idle_timeout_ms,
        })
        .await;
    let Err(err) = created else {
        let mut forward = message;
        forward.payload = serde_json::json!(TunnelOpenPayload {
            session_token: String::new(),
            tunnel_id: Some(tunnel_id.clone()),
            direction: payload.direction,
            listen_addr: payload.listen_addr,
            listen_port: payload.listen_port,
            target_host: payload.target_host,
            target_port: payload.target_port,
            idle_timeout_ms: payload.idle_timeout_ms,
            allow_non_loopback_listen: payload.allow_non_loopback_listen,
            allow_non_loopback_target: payload.allow_non_loopback_target,
        });
        forward.request_id = Some(request_id_value.clone());
        forward.session_id = Some(session.session_id.clone());
        if !send_text(&host_tx, forward) {
            let _ = state
                .inner
                .fail_tunnel(&tunnel_id, ErrorCode::HostDisconnected.message())
                .await;
            send_error(
                tx,
                Some(request_id_value),
                Some(session.session_id),
                ErrorCode::HostDisconnected,
                ErrorCode::HostDisconnected.message(),
            );
        }
        return;
    };
    send_error(
        tx,
        request_id,
        Some(session.session_id),
        err.code,
        &err.message,
    );
}

async fn handle_tunnel_status(state: &AppState, tx: &Tx, message: WireMessage) {
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
    let payload = match message.payload_as::<TunnelStatusPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid tunnel.status payload: {err}"),
            );
            return;
        }
    };
    let Some(tunnels) = state
        .inner
        .tunnels_for_session_if_valid(
            &session_id,
            &payload.session_token,
            payload.tunnel_id.as_deref(),
        )
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
    let result = WireMessage::new(
        TYPE_TUNNEL_STATUS_RESULT,
        request_id,
        Some(session_id),
        TunnelStatusResultPayload { ok: true, tunnels },
    )
    .expect("tunnel status result serializes");
    send_text(tx, result);
}

async fn handle_tunnel_close(state: &AppState, tx: &Tx, message: WireMessage) {
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
    let payload = match message.payload_as::<TunnelClosePayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid tunnel.close payload: {err}"),
            );
            return;
        }
    };
    let Some(tunnel) = state
        .inner
        .tunnel_if_valid(&payload.tunnel_id, &payload.session_token)
        .await
    else {
        send_error(
            tx,
            request_id,
            Some(session_id),
            ErrorCode::SessionExpired,
            "tunnel is not active",
        );
        return;
    };
    let closed = state
        .inner
        .close_tunnel_if_valid(
            &payload.tunnel_id,
            &payload.session_token,
            "controller_close",
        )
        .await
        .expect("tunnel existed before close");
    if let Some(host_tx) = state
        .inner
        .host_tx(&tunnel.host_id, &tunnel.connection_id)
        .await
    {
        send_text(&host_tx, message.clone());
    }
    let result = WireMessage::new(
        TYPE_TUNNEL_CLOSE_RESULT,
        request_id,
        Some(session_id),
        TunnelCloseResultPayload {
            ok: true,
            tunnel: closed,
        },
    )
    .expect("tunnel close result serializes");
    send_text(tx, result);
}

async fn handle_tunnel_stream_open(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let payload = match message.payload_as::<TunnelStreamOpenPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                message.session_id.clone(),
                ErrorCode::InternalError,
                &format!("invalid tunnel.stream_open payload: {err}"),
            );
            return;
        }
    };
    let route = match state
        .inner
        .add_tunnel_stream(
            &payload.tunnel_id,
            payload.stream_id.clone(),
            rcw_common::protocol::TunnelEndpointSide::Controller,
        )
        .await
    {
        Ok(route) => route,
        Err(err) => {
            send_error(tx, request_id, message.session_id, err.code, &err.message);
            return;
        }
    };
    let Some(host_tx) = state
        .inner
        .host_tx(&route.host_id, &route.connection_id)
        .await
    else {
        let _ = state
            .inner
            .close_tunnel_stream(&payload.tunnel_id, &payload.stream_id)
            .await;
        send_error(
            tx,
            request_id,
            Some(route.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };
    send_text(&host_tx, message);
}

async fn handle_tunnel_stream_eof(state: &AppState, tx: &Tx, message: WireMessage) {
    relay_tunnel_stream_control(
        state,
        tx,
        message,
        TYPE_TUNNEL_STREAM_EOF,
        false,
        rcw_common::protocol::TunnelEndpointSide::Controller,
    )
    .await;
}

async fn handle_tunnel_stream_open_result(state: &AppState, tx: &Tx, message: WireMessage) {
    let request_id = message.request_id.clone();
    let payload = match message.payload_as::<rcw_common::protocol::TunnelStreamOpenResultPayload>()
    {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                message.session_id.clone(),
                ErrorCode::InternalError,
                &format!("invalid tunnel.stream_open_result payload: {err}"),
            );
            return;
        }
    };
    let route = if payload.ok {
        state
            .inner
            .tunnel_stream(&payload.tunnel_id, &payload.stream_id)
            .await
    } else {
        state
            .inner
            .close_tunnel_stream(&payload.tunnel_id, &payload.stream_id)
            .await
    };
    let Some(route) = route else {
        return;
    };
    if let Some(host_tx) = state
        .inner
        .host_tx(&route.host_id, &route.connection_id)
        .await
    {
        send_text(&host_tx, message);
    }
}

async fn handle_tunnel_stream_reset(state: &AppState, tx: &Tx, message: WireMessage) {
    relay_tunnel_stream_control(
        state,
        tx,
        message,
        TYPE_TUNNEL_STREAM_RESET,
        true,
        rcw_common::protocol::TunnelEndpointSide::Controller,
    )
    .await;
}

async fn relay_tunnel_stream_control(
    state: &AppState,
    tx: &Tx,
    message: WireMessage,
    expected_kind: &str,
    remove_stream: bool,
    from_side: rcw_common::protocol::TunnelEndpointSide,
) {
    let request_id = message.request_id.clone();
    let payload = match message.payload_as::<TunnelStreamControlPayload>() {
        Ok(payload) => payload,
        Err(err) => {
            send_error(
                tx,
                request_id,
                message.session_id.clone(),
                ErrorCode::InternalError,
                &format!("invalid {expected_kind} payload: {err}"),
            );
            return;
        }
    };
    let route = if remove_stream {
        state
            .inner
            .close_tunnel_stream(&payload.tunnel_id, &payload.stream_id)
            .await
    } else {
        state
            .inner
            .mark_tunnel_stream_eof(&payload.tunnel_id, &payload.stream_id, from_side)
            .await
            .map(|(route, _)| route)
    };
    let Some(route) = route else {
        return;
    };
    if let Some(host_tx) = state
        .inner
        .host_tx(&route.host_id, &route.connection_id)
        .await
    {
        send_text(&host_tx, message);
    }
}

fn validate_tunnel_open_payload(
    payload: &TunnelOpenPayload,
) -> Result<(), rcw_common::protocol::ErrorPayload> {
    if payload.listen_addr.trim().is_empty() || payload.target_host.trim().is_empty() {
        return Err(rcw_common::protocol::ErrorPayload {
            code: ErrorCode::InvalidPath,
            message: "tunnel listen and target addresses are required".to_owned(),
        });
    }
    if payload.listen_port == 0 {
        return Err(rcw_common::protocol::ErrorPayload {
            code: ErrorCode::InvalidPath,
            message: "tunnel listen_port must be non-zero".to_owned(),
        });
    }
    if payload.target_port == 0 {
        return Err(rcw_common::protocol::ErrorPayload {
            code: ErrorCode::InvalidPath,
            message: "tunnel target_port must be non-zero".to_owned(),
        });
    }
    if !payload.allow_non_loopback_listen && !is_loopback_name(&payload.listen_addr) {
        return Err(rcw_common::protocol::ErrorPayload {
            code: ErrorCode::PermissionDenied,
            message: "tunnel listen address must be loopback unless explicitly allowed".to_owned(),
        });
    }
    if !payload.allow_non_loopback_target && !is_loopback_name(&payload.target_host) {
        return Err(rcw_common::protocol::ErrorPayload {
            code: ErrorCode::PermissionDenied,
            message: "tunnel target host must be loopback unless explicitly allowed".to_owned(),
        });
    }
    Ok(())
}

fn is_loopback_name(value: &str) -> bool {
    value.eq_ignore_ascii_case("localhost")
        || value == "127.0.0.1"
        || value == "::1"
        || value.starts_with("127.")
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

    let host = if let Some(host_id) = open.host_id.as_deref() {
        let Some(host) = state.inner.host(host_id).await else {
            send_error(
                tx,
                request_id,
                None,
                ErrorCode::HostNotFound,
                "host id is not online",
            );
            return;
        };
        if host.machine_id != open.machine_id {
            send_error(
                tx,
                request_id,
                None,
                ErrorCode::HostNotFound,
                "host id does not match machine id",
            );
            return;
        }
        host
    } else {
        match state.inner.host_for_machine_id(&open.machine_id).await {
            HostLookup::Found(host) => host,
            HostLookup::NotFound => {
                send_error(
                    tx,
                    request_id,
                    None,
                    ErrorCode::HostNotFound,
                    ErrorCode::HostNotFound.message(),
                );
                return;
            }
            HostLookup::Ambiguous(matches) => {
                send_error(
                    tx,
                    request_id,
                    None,
                    ErrorCode::HostBusy,
                    &format!(
                        "short machine ID {} matches {} online hosts; pass host_id to select one",
                        open.machine_id,
                        matches.len()
                    ),
                );
                return;
            }
        }
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

    if !open.force_reconnect && state.inner.host_has_active_session(&host.host_id).await {
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
                host_id: host.host_id.clone(),
                machine_id: open.machine_id.clone(),
                connection_id: host.connection_id.clone(),
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
    let host_online = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
        .is_some();
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

    if let Some(host_tx) = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
    {
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
            host_id: Some(session.host_id),
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

    let Some(host_tx) = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
    else {
        send_error(
            tx,
            request_id,
            Some(session.session_id.clone()),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };

    message.session_id = Some(session.session_id.clone());
    state
        .inner
        .track_request_route(request_id_value.clone(), &session, tx.clone())
        .await;
    if !send_text(&host_tx, message) {
        state.inner.clear_request_route(&request_id_value).await;
        send_error(
            tx,
            request_id,
            Some(session.session_id.clone()),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    }

    audit(
        state,
        AuditEvent {
            machine_id: Some(session.machine_id),
            host_id: Some(session.host_id),
            session_id: Some(session.session_id),
            request_id,
            command: Some(payload.command),
            audit_label: payload.audit_label,
            result: Some("relayed".to_owned()),
            ..AuditEvent::new("server", "command.relay")
        },
    );
}

async fn handle_command_start(state: &AppState, tx: &Tx, mut message: WireMessage) {
    let Some(task_id) = message.request_id.clone() else {
        send_error(
            tx,
            None,
            message.session_id.clone(),
            ErrorCode::InternalError,
            "command.start requires request_id",
        );
        return;
    };
    let Some(session_id) = message.session_id.clone() else {
        send_error(
            tx,
            Some(task_id),
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
                Some(task_id),
                Some(session_id),
                ErrorCode::InternalError,
                &format!("invalid command.start payload: {err}"),
            );
            return;
        }
    };
    if payload.command != rcw_common::protocol::COMMAND_EXEC {
        send_error(
            tx,
            Some(task_id),
            Some(session_id),
            ErrorCode::UnsupportedCommand,
            "command.start only supports exec",
        );
        return;
    }

    let session = state
        .inner
        .session_if_valid(&session_id, &payload.session_token)
        .await;
    let Some(session) = session else {
        send_error(
            tx,
            Some(task_id),
            Some(session_id),
            ErrorCode::SessionExpired,
            ErrorCode::SessionExpired.message(),
        );
        return;
    };
    let Some(host_tx) = state
        .inner
        .host_tx(&session.host_id, &session.connection_id)
        .await
    else {
        send_error(
            tx,
            Some(task_id),
            Some(session.session_id.clone()),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    };

    let snapshot = state
        .inner
        .create_exec_job(CreateExecJob {
            task_id: task_id.clone(),
            host_id: session.host_id.clone(),
            connection_id: session.connection_id.clone(),
            session_id: session.session_id.clone(),
            session_token: payload.session_token.clone(),
            started_at: rcw_common::audit::now_rfc3339(),
        })
        .await;
    message.kind = TYPE_COMMAND_REQUEST.to_owned();
    message.session_id = Some(session.session_id.clone());
    state
        .inner
        .track_detached_request_route(task_id.clone(), &session, tx.clone())
        .await;
    if !send_text(&host_tx, message) {
        let error = rcw_common::protocol::ErrorPayload {
            code: ErrorCode::HostDisconnected,
            message: ErrorCode::HostDisconnected.message().to_owned(),
        };
        state.inner.fail_exec_job(&task_id, error).await;
        state.inner.clear_request_route(&task_id).await;
        send_error(
            tx,
            Some(task_id),
            Some(session.session_id),
            ErrorCode::HostDisconnected,
            ErrorCode::HostDisconnected.message(),
        );
        return;
    }

    let result = WireMessage::new(
        TYPE_COMMAND_START_RESULT,
        Some(task_id.clone()),
        Some(session.session_id.clone()),
        snapshot,
    )
    .expect("command start result serializes");
    send_text(tx, result);

    audit(
        state,
        AuditEvent {
            machine_id: Some(session.machine_id),
            host_id: Some(session.host_id),
            session_id: Some(session.session_id),
            request_id: Some(task_id),
            command: Some(payload.command),
            audit_label: payload.audit_label,
            result: Some("started".to_owned()),
            summary: Some("detached exec job".to_owned()),
            ..AuditEvent::new("server", "command.start")
        },
    );
}
