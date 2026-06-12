use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    audit::{append_jsonl, AuditEvent},
    config,
    ids::{new_session_id, new_session_token, token_label},
    protocol::{
        ControlOpenPayload, ControlOpenResultPayload, ErrorCode, ErrorPayload,
        HostAuthRequestPayload, HostAuthResultPayload, HostHelloAckPayload, HostHelloPayload,
        HostSessionClosedPayload, HostSessionOpenedPayload, SessionClosePayload,
        SessionCloseResultPayload, SessionStatusPayload, SessionStatusResultPayload, WireMessage,
        PROTOCOL_VERSION, TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT, TYPE_COMMAND_REQUEST,
        TYPE_CONTROL_OPEN, TYPE_CONTROL_OPEN_RESULT, TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR,
        TYPE_HOST_AUTH_REQUEST, TYPE_HOST_AUTH_RESULT, TYPE_HOST_HELLO, TYPE_HOST_HELLO_ACK,
        TYPE_HOST_SESSION_CLOSED, TYPE_HOST_SESSION_OPENED, TYPE_SESSION_CLOSE,
        TYPE_SESSION_CLOSE_RESULT, TYPE_SESSION_STATUS, TYPE_SESSION_STATUS_RESULT,
        TYPE_UPLOAD_COMPLETE,
    },
    transfer::BinaryFrame,
};
use serde_json::json;
use tokio::{
    net::TcpListener,
    sync::{mpsc, Mutex},
};
use tracing::{debug, error, info, warn};

const HEARTBEAT_INTERVAL_MS: u64 = 15_000;
const OUTBOUND_QUEUE_CAPACITY: usize = 128;

type Tx = mpsc::Sender<Outbound>;

enum Outbound {
    Text(WireMessage),
    Binary(Vec<u8>),
}

#[derive(Clone)]
struct AppState {
    inner: Arc<ServerState>,
    control_token: Arc<String>,
    audit_path: Arc<PathBuf>,
}

struct ServerState {
    hosts: Mutex<HashMap<String, HostConn>>,
    sessions: Mutex<HashMap<String, SessionState>>,
    pending_open: Mutex<HashMap<String, PendingOpen>>,
    request_routes: Mutex<HashMap<String, String>>,
    request_controllers: Mutex<HashMap<String, Tx>>,
    host_registrations: Mutex<HashMap<String, VecDeque<Instant>>>,
    auth_attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

#[derive(Clone)]
struct HostConn {
    tx: Tx,
    totp_period_seconds: u64,
}

#[derive(Clone)]
struct SessionState {
    session_id: String,
    session_token: String,
    machine_id: String,
    controller_tx: Option<Tx>,
    last_seen: String,
}

#[derive(Clone)]
struct PendingOpen {
    machine_id: String,
    controller_tx: Tx,
    controller_label: String,
}

impl ServerState {
    fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending_open: Mutex::new(HashMap::new()),
            request_routes: Mutex::new(HashMap::new()),
            request_controllers: Mutex::new(HashMap::new()),
            host_registrations: Mutex::new(HashMap::new()),
            auth_attempts: Mutex::new(HashMap::new()),
        }
    }

    async fn register_host(&self, machine_id: String, tx: Tx, totp_period_seconds: u64) {
        let mut hosts = self.hosts.lock().await;
        hosts.insert(
            machine_id,
            HostConn {
                tx,
                totp_period_seconds,
            },
        );
    }

    async fn host(&self, machine_id: &str) -> Option<HostConn> {
        let hosts = self.hosts.lock().await;
        hosts.get(machine_id).cloned()
    }

    async fn host_tx(&self, machine_id: &str) -> Option<Tx> {
        self.host(machine_id).await.map(|host| host.tx)
    }

    async fn host_has_active_session(&self, machine_id: &str) -> bool {
        let sessions = self.sessions.lock().await;
        sessions
            .values()
            .any(|session| session.machine_id == machine_id)
    }

    async fn insert_pending_open(&self, request_id: String, pending: PendingOpen) {
        let mut pending_open = self.pending_open.lock().await;
        pending_open.insert(request_id, pending);
    }

    async fn take_pending_open(&self, request_id: &str) -> Option<PendingOpen> {
        let mut pending_open = self.pending_open.lock().await;
        pending_open.remove(request_id)
    }

    async fn remove_pending_open(&self, request_id: &str) {
        let mut pending_open = self.pending_open.lock().await;
        pending_open.remove(request_id);
    }

    async fn create_session(
        &self,
        session_id: String,
        session_token: String,
        machine_id: String,
        controller_tx: Tx,
    ) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            session_id.clone(),
            SessionState {
                session_id,
                session_token,
                machine_id,
                controller_tx: Some(controller_tx),
                last_seen: rcw_common::audit::now_rfc3339(),
            },
        );
    }

    async fn session_if_valid(
        &self,
        session_id: &str,
        session_token: &str,
    ) -> Option<SessionState> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .filter(|session| session.session_token == session_token)
            .cloned()
    }

    async fn remove_session_if_valid(
        &self,
        session_id: &str,
        session_token: &str,
    ) -> Option<SessionState> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get(session_id) {
            Some(session) if session.session_token == session_token => sessions.remove(session_id),
            _ => None,
        }
    }

    async fn bind_controller_to_session(
        &self,
        session_id: &str,
        session_token: &str,
        controller_tx: Tx,
    ) -> Option<SessionState> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(session_id) {
            Some(session) if session.session_token == session_token => {
                session.controller_tx = Some(controller_tx);
                session.last_seen = rcw_common::audit::now_rfc3339();
                Some(session.clone())
            }
            _ => None,
        }
    }

    async fn track_request_route(&self, request_id: String, session_id: String, tx: Tx) {
        let mut routes = self.request_routes.lock().await;
        routes.insert(request_id.clone(), session_id);
        drop(routes);

        let mut controllers = self.request_controllers.lock().await;
        controllers.insert(request_id, tx);
    }

    async fn clear_request_route(&self, request_id: &str) {
        let mut routes = self.request_routes.lock().await;
        routes.remove(request_id);
        drop(routes);

        let mut controllers = self.request_controllers.lock().await;
        controllers.remove(request_id);
    }

    async fn request_session_id(&self, request_id: &str) -> Option<String> {
        let routes = self.request_routes.lock().await;
        routes.get(request_id).cloned()
    }

    async fn controller_for_request(&self, request_id: &str) -> Option<Tx> {
        let controllers = self.request_controllers.lock().await;
        controllers.get(request_id).cloned()
    }

    async fn session_controller_for_machine(
        &self,
        session_id: &str,
        machine_id: &str,
    ) -> Option<Tx> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .filter(|session| session.machine_id == machine_id)
            .and_then(|session| session.controller_tx.clone())
    }

    async fn command_route(&self, session_id: &str) -> Option<(String, String)> {
        let sessions = self.sessions.lock().await;
        let session = sessions.get(session_id)?;
        Some((session.machine_id.clone(), session.session_id.clone()))
    }

    async fn unregister_host(&self, machine_id: &str) -> Vec<SessionState> {
        let mut hosts = self.hosts.lock().await;
        hosts.remove(machine_id);
        drop(hosts);

        let removed_sessions = {
            let mut sessions = self.sessions.lock().await;
            let session_ids = sessions
                .values()
                .filter(|session| session.machine_id == machine_id)
                .map(|session| session.session_id.clone())
                .collect::<Vec<_>>();
            session_ids
                .into_iter()
                .filter_map(|session_id| sessions.remove(&session_id))
                .collect::<Vec<_>>()
        };

        let removed_session_ids = removed_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        let removed_request_ids = {
            let mut routes = self.request_routes.lock().await;
            let request_ids = routes
                .iter()
                .filter(|(_, session_id)| removed_session_ids.contains(*session_id))
                .map(|(request_id, _)| request_id.clone())
                .collect::<Vec<_>>();
            for request_id in &request_ids {
                routes.remove(request_id);
            }
            request_ids
        };
        if !removed_request_ids.is_empty() {
            let mut controllers = self.request_controllers.lock().await;
            for request_id in removed_request_ids {
                controllers.remove(&request_id);
            }
        }

        removed_sessions
    }

    async fn allow_host_registration(&self, machine_id: &str) -> bool {
        allow_rate_limit(
            &self.host_registrations,
            machine_id,
            20,
            Duration::from_secs(60),
        )
        .await
    }

    async fn allow_auth_attempt(&self, machine_id: &str) -> bool {
        allow_rate_limit(&self.auth_attempts, machine_id, 12, Duration::from_secs(60)).await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_filter = std::env::var("RCW_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .compact()
        .init();

    let control_token = config::control_token(None)
        .context("RCW_CONTROL_TOKEN must be set before starting rcw-server")?;
    let bind_addr: SocketAddr = config::bind_addr()
        .parse()
        .context("RCW_BIND_ADDR must be a socket address")?;
    let audit_path = PathBuf::from(config::server_audit_log_path());

    let state = AppState {
        inner: Arc::new(ServerState::new()),
        control_token: Arc::new(control_token),
        audit_path: Arc::new(audit_path),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws/host", get(host_ws))
        .route("/ws/control", get(control_ws))
        .with_state(state);

    let listener = TcpListener::bind(bind_addr).await?;
    info!("rcw-server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "rcw-server",
        "protocol_version": PROTOCOL_VERSION,
    }))
}

async fn host_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_host_socket(socket, state))
}

async fn control_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_control_socket(socket, state))
}

fn outbound_channel() -> (Tx, mpsc::Receiver<Outbound>) {
    mpsc::channel(OUTBOUND_QUEUE_CAPACITY)
}

fn spawn_writer(
    mut sender: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Outbound>,
    peer: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    let Some(message) = maybe else {
                        break;
                    };
                    let message = match outbound_to_ws_message(message, peer) {
                        Some(message) => message,
                        None => continue,
                    };
                    if sender.send(message).await.is_err() {
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    if sender.send(Message::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn outbound_to_ws_message(outbound: Outbound, peer: &str) -> Option<Message> {
    match outbound {
        Outbound::Text(message) => match serde_json::to_string(&message) {
            Ok(text) => Some(Message::Text(text)),
            Err(err) => {
                warn!("failed to serialize outbound {peer} message: {err}");
                None
            }
        },
        Outbound::Binary(bytes) => Some(Message::Binary(bytes)),
    }
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

    if !state.inner.allow_host_registration(&hello.machine_id).await {
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

    state
        .inner
        .register_host(
            hello.machine_id.clone(),
            tx.clone(),
            hello.totp_period_seconds,
        )
        .await;

    audit(
        &state,
        AuditEvent {
            machine_id: Some(hello.machine_id.clone()),
            summary: Some(format!(
                "host connected version={} os={}",
                hello.host_version, hello.os
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

    info!("host {} connected", hello.machine_id);

    while let Some(frame) = receiver.next().await {
        match frame {
            Ok(Message::Text(text)) => match serde_json::from_str::<WireMessage>(&text) {
                Ok(message) => handle_host_message(&state, &hello.machine_id, message).await,
                Err(err) => warn!("invalid host message from {}: {err}", hello.machine_id),
            },
            Ok(Message::Binary(bytes)) => {
                relay_host_binary_response(&state, &hello.machine_id, bytes).await;
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                warn!("host websocket error for {}: {err}", hello.machine_id);
                break;
            }
        }
    }

    writer.abort();
    unregister_host(&state, &hello.machine_id).await;
}

async fn handle_host_message(state: &AppState, machine_id: &str, message: WireMessage) {
    match message.kind.as_str() {
        TYPE_HOST_AUTH_RESULT => handle_host_auth_result(state, machine_id, message).await,
        TYPE_COMMAND_OUTPUT
        | TYPE_COMMAND_COMPLETE
        | TYPE_UPLOAD_COMPLETE
        | TYPE_DOWNLOAD_COMPLETE
        | TYPE_ERROR => relay_host_response(state, machine_id, message).await,
        other => debug!("ignored host message type {other} from {machine_id}"),
    }
}

async fn handle_host_auth_result(state: &AppState, machine_id: &str, message: WireMessage) {
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

    if pending.machine_id != machine_id {
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
                machine_id: Some(machine_id.to_owned()),
                request_id: Some(request_id),
                result: Some("failed".to_owned()),
                summary: Some(code.message().to_owned()),
                ..AuditEvent::new("server", "session.auth")
            },
        );
        return;
    }

    let session_id = new_session_id();
    let session_token = new_session_token();
    state
        .inner
        .create_session(
            session_id.clone(),
            session_token.clone(),
            machine_id.to_owned(),
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
            machine_id: machine_id.to_owned(),
        },
    )
    .expect("open result serializes");
    send_text(&pending.controller_tx, result);

    if let Some(host_tx) = state.inner.host_tx(machine_id).await {
        let opened = WireMessage::new(
            TYPE_HOST_SESSION_OPENED,
            Some(request_id.clone()),
            Some(session_id.clone()),
            HostSessionOpenedPayload {
                session_id: session_id.clone(),
                controller_label: pending.controller_label,
            },
        )
        .expect("session opened serializes");
        send_text(&host_tx, opened);
    }

    audit(
        state,
        AuditEvent {
            machine_id: Some(machine_id.to_owned()),
            session_id: Some(session_id),
            request_id: Some(request_id),
            result: Some("ok".to_owned()),
            ..AuditEvent::new("server", "session.created")
        },
    );
}

async fn relay_host_response(state: &AppState, machine_id: &str, message: WireMessage) {
    let request_id = message.request_id.clone();
    let is_terminal = matches!(
        message.kind.as_str(),
        TYPE_COMMAND_COMPLETE | TYPE_UPLOAD_COMPLETE | TYPE_DOWNLOAD_COMPLETE | TYPE_ERROR
    );
    let Some(session_id) = message.session_id.clone() else {
        warn!("host response without session_id from {machine_id}");
        return;
    };
    let mut controller_tx = if let Some(request_id) = &request_id {
        state.inner.controller_for_request(request_id).await
    } else {
        None
    };
    if controller_tx.is_none() {
        controller_tx = state
            .inner
            .session_controller_for_machine(&session_id, machine_id)
            .await;
    }
    debug!(
        kind = %message.kind,
        request_id = ?request_id,
        session_id = %session_id,
        machine_id = %machine_id,
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

async fn relay_host_binary_response(state: &AppState, machine_id: &str, bytes: Vec<u8>) {
    let frame = match BinaryFrame::decode(&bytes) {
        Ok(frame) => frame,
        Err(err) => {
            warn!("invalid binary frame from host {machine_id}: {err}");
            return;
        }
    };
    let session_id = state.inner.request_session_id(&frame.request_id).await;
    let Some(session_id) = session_id else {
        warn!(
            "host binary frame for unknown request {} from {}",
            frame.request_id, machine_id
        );
        return;
    };
    let mut controller_tx = state.inner.controller_for_request(&frame.request_id).await;
    if controller_tx.is_none() {
        controller_tx = state
            .inner
            .session_controller_for_machine(&session_id, machine_id)
            .await;
    }
    debug!(
        request_id = %frame.request_id,
        session_id = %session_id,
        machine_id = %machine_id,
        bytes = bytes.len(),
        has_controller = controller_tx.is_some(),
        "relaying host binary response"
    );
    if let Some(tx) = controller_tx {
        send_binary(&tx, bytes);
    };
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
                warn!("control websocket error: {err}");
                break;
            }
        }
    }

    writer.abort();
}

async fn handle_control_message(state: &AppState, tx: &Tx, message: WireMessage) {
    match message.kind.as_str() {
        TYPE_CONTROL_OPEN => handle_control_open(state, tx, message).await,
        TYPE_SESSION_STATUS => handle_session_status(state, tx, message).await,
        TYPE_SESSION_CLOSE => handle_session_close(state, tx, message).await,
        TYPE_COMMAND_REQUEST => handle_command_request(state, tx, message).await,
        other => send_error(
            tx,
            message.request_id,
            message.session_id,
            ErrorCode::UnsupportedCommand,
            &format!("unsupported message type: {other}"),
        ),
    }
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

    if state.inner.host_has_active_session(&open.machine_id).await {
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
    let payload = match message.payload_as::<rcw_common::protocol::CommandRequestPayload>() {
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

async fn unregister_host(state: &AppState, machine_id: &str) {
    let removed_sessions = state.inner.unregister_host(machine_id).await;

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
                machine_id: Some(machine_id.to_owned()),
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
            result: Some("ok".to_owned()),
            ..AuditEvent::new("server", "host.disconnected")
        },
    );
    info!("host {machine_id} disconnected");
}

fn send_error(
    tx: &Tx,
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) {
    send_text(tx, make_error(request_id, session_id, code, message));
}

fn make_error(
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) -> WireMessage {
    WireMessage::new(
        TYPE_ERROR,
        request_id,
        session_id,
        ErrorPayload {
            code,
            message: message.to_owned(),
        },
    )
    .expect("error payload serializes")
}

fn audit(state: &AppState, event: AuditEvent) {
    if let Err(err) = append_jsonl(state.audit_path.as_ref(), &event) {
        error!("failed to write server audit log: {err}");
    }
}

fn send_text(tx: &Tx, message: WireMessage) -> bool {
    send_outbound(tx, Outbound::Text(message))
}

fn send_binary(tx: &Tx, bytes: Vec<u8>) -> bool {
    send_outbound(tx, Outbound::Binary(bytes))
}

fn send_outbound(tx: &Tx, outbound: Outbound) -> bool {
    match tx.try_send(outbound) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!("outbound websocket queue full; dropping message");
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            debug!("outbound websocket closed; dropping message");
            false
        }
    }
}

async fn allow_rate_limit(
    table: &Mutex<HashMap<String, VecDeque<Instant>>>,
    key: &str,
    max_events: usize,
    window: Duration,
) -> bool {
    let now = Instant::now();
    let mut table = table.lock().await;
    let events = table.entry(key.to_owned()).or_default();
    while events
        .front()
        .map(|instant| now.duration_since(*instant) > window)
        .unwrap_or(false)
    {
        events.pop_front();
    }
    if events.len() >= max_events {
        return false;
    }
    events.push_back(now);
    true
}
