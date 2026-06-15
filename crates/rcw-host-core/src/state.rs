use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rcw_common::{
    audit::now_rfc3339,
    protocol::{TunnelDirection, TunnelInfo, TunnelStatus},
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

const AUTH_HISTORY_LIMIT: usize = 50;
const TASK_HISTORY_LIMIT: usize = 100;
const TUNNEL_HISTORY_LIMIT: usize = 100;
const RECENT_ERROR_LIMIT: usize = 50;
const EVENT_HISTORY_LIMIT: usize = 200;
const EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostListenerStatus {
    Stopped,
    Connecting,
    Connected,
    Reconnecting,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostTaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostTransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostEventKind {
    ListenerStatusChanged,
    HostConnected,
    HostDisconnected,
    AuthRequested,
    SessionOpened,
    SessionClosed,
    CommandStarted,
    CommandCompleted,
    CommandCancelRequested,
    TransferStarted,
    TransferProgress,
    TransferCompleted,
    TunnelOpened,
    TunnelClosed,
    ErrorRecorded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostEvent {
    pub time: String,
    pub kind: HostEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostListenerSnapshot {
    pub status: HostListenerStatus,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostTotpSnapshot {
    pub current_code: String,
    pub period_seconds: u64,
    pub remaining_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPowerSnapshot {
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostSessionSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_close_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostAuthRequestSnapshot {
    pub request_id: String,
    pub controller_label: String,
    pub at: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostCommandTaskSnapshot {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub command: String,
    pub status: HostTaskStatus,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostTransferTaskSnapshot {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub direction: HostTransferDirection,
    pub status: HostTaskStatus,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub bytes_transferred: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostTunnelSnapshot {
    pub tunnel_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub direction: TunnelDirection,
    pub listen_addr: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub status: TunnelStatus,
    pub opened_at: String,
    pub last_activity_at: String,
    pub active_streams: usize,
    pub total_streams: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
}

impl From<TunnelInfo> for HostTunnelSnapshot {
    fn from(info: TunnelInfo) -> Self {
        Self {
            tunnel_id: info.tunnel_id,
            session_id: (!info.session_id.is_empty()).then_some(info.session_id),
            direction: info.direction,
            listen_addr: info.listen_addr,
            listen_port: info.listen_port,
            target_host: info.target_host,
            target_port: info.target_port,
            status: info.status,
            opened_at: info.opened_at,
            last_activity_at: info.last_activity_at,
            active_streams: info.active_streams,
            total_streams: info.total_streams,
            close_reason: info.close_reason,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostErrorSnapshot {
    pub at: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSnapshot {
    pub listener: HostListenerSnapshot,
    pub server_url: String,
    pub machine_id: String,
    pub host_id: String,
    pub totp: HostTotpSnapshot,
    pub power: HostPowerSnapshot,
    pub audit_path: PathBuf,
    pub session: HostSessionSnapshot,
    pub auth_requests: Vec<HostAuthRequestSnapshot>,
    pub commands: Vec<HostCommandTaskSnapshot>,
    pub transfers: Vec<HostTransferTaskSnapshot>,
    pub tunnels: Vec<HostTunnelSnapshot>,
    pub recent_errors: Vec<HostErrorSnapshot>,
    pub events: Vec<HostEvent>,
}

#[derive(Debug, Clone)]
pub(crate) struct HostStateMetadata {
    pub(crate) server_url: String,
    pub(crate) machine_id: String,
    pub(crate) host_id: String,
    pub(crate) totp_period_seconds: u64,
    pub(crate) audit_path: PathBuf,
    pub(crate) power: HostPowerSnapshot,
}

#[derive(Debug)]
struct HostStateInner {
    snapshot: HostSnapshot,
    command_positions: HashMap<String, usize>,
    transfer_positions: HashMap<String, usize>,
    tunnel_positions: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct HostStateHandle {
    inner: Arc<Mutex<HostStateInner>>,
    events: broadcast::Sender<HostEvent>,
}

impl HostStateHandle {
    pub(crate) fn new(metadata: HostStateMetadata) -> Self {
        let now = now_rfc3339();
        let snapshot = HostSnapshot {
            listener: HostListenerSnapshot {
                status: HostListenerStatus::Stopped,
                updated_at: now,
                last_error: None,
            },
            server_url: metadata.server_url,
            machine_id: metadata.machine_id,
            host_id: metadata.host_id,
            totp: HostTotpSnapshot {
                current_code: String::new(),
                period_seconds: metadata.totp_period_seconds,
                remaining_seconds: metadata.totp_period_seconds,
            },
            power: metadata.power,
            audit_path: metadata.audit_path,
            session: HostSessionSnapshot::default(),
            auth_requests: Vec::new(),
            commands: Vec::new(),
            transfers: Vec::new(),
            tunnels: Vec::new(),
            recent_errors: Vec::new(),
            events: Vec::new(),
        };
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(HostStateInner {
                snapshot,
                command_positions: HashMap::new(),
                transfer_positions: HashMap::new(),
                tunnel_positions: HashMap::new(),
            })),
            events,
        }
    }

    pub(crate) fn snapshot(&self, totp: HostTotpSnapshot) -> HostSnapshot {
        let mut snapshot = self
            .inner
            .lock()
            .expect("host state lock poisoned")
            .snapshot
            .clone();
        snapshot.totp = totp;
        snapshot
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<HostEvent> {
        self.events.subscribe()
    }

    pub(crate) fn record_listener_status(
        &self,
        status: HostListenerStatus,
        reason: Option<String>,
    ) {
        let time = now_rfc3339();
        let summary = reason
            .as_ref()
            .map(|reason| redact_reason(reason))
            .or_else(|| Some(format!("{status:?}").to_lowercase()));
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::ListenerStatusChanged,
                request_id: None,
                session_id: None,
                command: None,
                status: Some(format!("{status:?}").to_lowercase()),
                summary: summary.clone(),
            },
            |inner| {
                inner.snapshot.listener.status = status;
                inner.snapshot.listener.updated_at = time.clone();
                inner.snapshot.listener.last_error =
                    reason.clone().map(|reason| redact_reason(&reason));
                if matches!(status, HostListenerStatus::Error) {
                    push_error(
                        &mut inner.snapshot,
                        HostErrorSnapshot {
                            at: time,
                            summary: reason
                                .as_deref()
                                .map(redact_reason)
                                .unwrap_or_else(|| "listener error".to_owned()),
                            request_id: None,
                            session_id: None,
                        },
                    );
                }
            },
        );
    }

    pub(crate) fn record_connected(&self) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::HostConnected,
                request_id: None,
                session_id: None,
                command: None,
                status: Some("connected".to_owned()),
                summary: Some("host websocket connected".to_owned()),
            },
            |inner| {
                inner.snapshot.listener.status = HostListenerStatus::Connected;
                inner.snapshot.listener.updated_at = time;
                inner.snapshot.listener.last_error = None;
            },
        );
    }

    pub(crate) fn record_disconnected(&self, session_id: Option<String>) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::HostDisconnected,
                request_id: None,
                session_id: session_id.clone(),
                command: None,
                status: Some("disconnected".to_owned()),
                summary: Some("host websocket disconnected".to_owned()),
            },
            |inner| {
                inner.snapshot.listener.updated_at = time;
            },
        );
    }

    pub(crate) fn record_auth_request(
        &self,
        request_id: String,
        controller_label: String,
        ok: bool,
    ) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::AuthRequested,
                request_id: Some(request_id.clone()),
                session_id: None,
                command: None,
                status: Some(if ok { "ok" } else { "failed" }.to_owned()),
                summary: Some("session auth request".to_owned()),
            },
            |inner| {
                inner.snapshot.auth_requests.push(HostAuthRequestSnapshot {
                    request_id,
                    controller_label: compact_label(controller_label),
                    at: time,
                    ok,
                });
                trim_vec(&mut inner.snapshot.auth_requests, AUTH_HISTORY_LIMIT);
            },
        );
    }

    pub(crate) fn record_session_opened(&self, session_id: String, controller_label: String) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::SessionOpened,
                request_id: None,
                session_id: Some(session_id.clone()),
                command: None,
                status: Some("active".to_owned()),
                summary: Some("session opened".to_owned()),
            },
            |inner| {
                inner.snapshot.session.active_session_id = Some(session_id);
                inner.snapshot.session.controller_label = Some(compact_label(controller_label));
                inner.snapshot.session.opened_at = Some(time);
                inner.snapshot.session.last_close_reason = None;
            },
        );
    }

    pub(crate) fn record_session_closed(&self, session_id: String, reason: String) {
        let time = now_rfc3339();
        let redacted_reason = redact_reason(&reason);
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::SessionClosed,
                request_id: None,
                session_id: Some(session_id.clone()),
                command: None,
                status: Some("closed".to_owned()),
                summary: Some(redacted_reason.clone()),
            },
            |inner| {
                inner.snapshot.session.active_session_id = None;
                inner.snapshot.session.controller_label = None;
                inner.snapshot.session.opened_at = None;
                inner.snapshot.session.last_closed_at = Some(time.clone());
                inner.snapshot.session.last_close_reason = Some(redacted_reason.clone());
                mark_session_tasks_closed(
                    &mut inner.snapshot,
                    &session_id,
                    &time,
                    "session_closed",
                );
            },
        );
    }

    pub(crate) fn record_command_started(
        &self,
        request_id: String,
        session_id: Option<String>,
        command: String,
    ) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::CommandStarted,
                request_id: Some(request_id.clone()),
                session_id: session_id.clone(),
                command: Some(command.clone()),
                status: Some("running".to_owned()),
                summary: Some(command_summary(&command)),
            },
            |inner| {
                upsert_command(
                    inner,
                    HostCommandTaskSnapshot {
                        request_id,
                        session_id,
                        command,
                        status: HostTaskStatus::Running,
                        started_at: time,
                        finished_at: None,
                        result: None,
                        duration_ms: None,
                        summary: None,
                    },
                );
            },
        );
    }

    pub(crate) fn record_command_completed(
        &self,
        request_id: String,
        session_id: Option<String>,
        command: String,
        status: HostTaskStatus,
        duration_ms: u64,
        result: String,
    ) {
        let time = now_rfc3339();
        let summary = Some(result.clone());
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::CommandCompleted,
                request_id: Some(request_id.clone()),
                session_id: session_id.clone(),
                command: Some(command.clone()),
                status: Some(task_status_str(status).to_owned()),
                summary,
            },
            |inner| {
                if let Some(command) = find_command_mut(inner, &request_id) {
                    command.status = status;
                    command.finished_at = Some(time.clone());
                    command.duration_ms = Some(duration_ms);
                    command.result = Some(result.clone());
                }
                if matches!(status, HostTaskStatus::Failed | HostTaskStatus::Cancelled) {
                    push_error(
                        &mut inner.snapshot,
                        HostErrorSnapshot {
                            at: time,
                            summary: format!("command {result}"),
                            request_id: Some(request_id),
                            session_id,
                        },
                    );
                }
            },
        );
    }

    pub(crate) fn record_cancel_requested(&self, request_id: String, session_id: Option<String>) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::CommandCancelRequested,
                request_id: Some(request_id.clone()),
                session_id: session_id.clone(),
                command: None,
                status: Some("cancel_requested".to_owned()),
                summary: Some("cancel requested".to_owned()),
            },
            |inner| {
                if let Some(command) = find_command_mut(inner, &request_id) {
                    command.status = HostTaskStatus::Cancelled;
                    command.finished_at = Some(time.clone());
                    command.result = Some("cancel_requested".to_owned());
                }
                if let Some(transfer) = find_transfer_mut(inner, &request_id) {
                    transfer.status = HostTaskStatus::Cancelled;
                    transfer.finished_at = Some(time);
                    transfer.result = Some("cancel_requested".to_owned());
                }
            },
        );
    }

    pub(crate) fn record_transfer_started(
        &self,
        request_id: String,
        session_id: Option<String>,
        direction: HostTransferDirection,
        size: Option<u64>,
    ) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::TransferStarted,
                request_id: Some(request_id.clone()),
                session_id: session_id.clone(),
                command: Some(
                    match direction {
                        HostTransferDirection::Upload => "upload.begin",
                        HostTransferDirection::Download => "download.begin",
                    }
                    .to_owned(),
                ),
                status: Some("running".to_owned()),
                summary: Some(transfer_summary(direction, size)),
            },
            |inner| {
                upsert_transfer(
                    inner,
                    HostTransferTaskSnapshot {
                        request_id,
                        session_id,
                        direction,
                        status: HostTaskStatus::Running,
                        started_at: time,
                        finished_at: None,
                        size,
                        bytes_transferred: 0,
                        result: None,
                        summary: None,
                    },
                );
            },
        );
    }

    pub(crate) fn record_transfer_progress(
        &self,
        request_id: String,
        session_id: Option<String>,
        bytes_transferred: u64,
    ) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time,
                kind: HostEventKind::TransferProgress,
                request_id: Some(request_id.clone()),
                session_id,
                command: None,
                status: Some("running".to_owned()),
                summary: Some(format!("bytes={bytes_transferred}")),
            },
            |inner| {
                if let Some(transfer) = find_transfer_mut(inner, &request_id) {
                    transfer.bytes_transferred = bytes_transferred;
                }
            },
        );
    }

    pub(crate) fn record_transfer_completed(
        &self,
        request_id: String,
        session_id: Option<String>,
        status: HostTaskStatus,
        bytes_transferred: Option<u64>,
        result: String,
    ) {
        let time = now_rfc3339();
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::TransferCompleted,
                request_id: Some(request_id.clone()),
                session_id: session_id.clone(),
                command: None,
                status: Some(task_status_str(status).to_owned()),
                summary: Some(result.clone()),
            },
            |inner| {
                if let Some(transfer) = find_transfer_mut(inner, &request_id) {
                    transfer.status = status;
                    transfer.finished_at = Some(time.clone());
                    transfer.result = Some(result.clone());
                    if let Some(bytes) = bytes_transferred {
                        transfer.bytes_transferred = bytes;
                    }
                }
                if matches!(status, HostTaskStatus::Failed | HostTaskStatus::Cancelled) {
                    push_error(
                        &mut inner.snapshot,
                        HostErrorSnapshot {
                            at: time,
                            summary: format!("transfer {result}"),
                            request_id: Some(request_id),
                            session_id,
                        },
                    );
                }
            },
        );
    }

    pub(crate) fn record_tunnel_opened(&self, info: TunnelInfo) {
        let time = now_rfc3339();
        let snapshot = HostTunnelSnapshot::from(info);
        let tunnel_id = snapshot.tunnel_id.clone();
        let session_id = snapshot.session_id.clone();
        self.apply(
            HostEvent {
                time,
                kind: HostEventKind::TunnelOpened,
                request_id: None,
                session_id,
                command: None,
                status: Some("active".to_owned()),
                summary: Some(format!("tunnel direction={:?}", snapshot.direction).to_lowercase()),
            },
            |inner| {
                upsert_tunnel(inner, snapshot);
            },
        );
        self.trim_tunnel_history(&tunnel_id);
    }

    pub(crate) fn record_tunnel_closed(
        &self,
        tunnel_id: String,
        session_id: Option<String>,
        reason: Option<String>,
    ) {
        let time = now_rfc3339();
        let summary = reason
            .as_deref()
            .map(redact_reason)
            .unwrap_or_else(|| "closed".to_owned());
        self.apply(
            HostEvent {
                time: time.clone(),
                kind: HostEventKind::TunnelClosed,
                request_id: None,
                session_id,
                command: None,
                status: Some("closed".to_owned()),
                summary: Some(summary.clone()),
            },
            |inner| {
                if let Some(tunnel) = find_tunnel_mut(inner, &tunnel_id) {
                    tunnel.status = TunnelStatus::Closed;
                    tunnel.last_activity_at = time;
                    tunnel.close_reason = Some(summary);
                }
            },
        );
    }

    fn apply<F>(&self, event: HostEvent, f: F)
    where
        F: FnOnce(&mut HostStateInner),
    {
        let emitted_event = event.clone();
        {
            let mut inner = self.inner.lock().expect("host state lock poisoned");
            f(&mut inner);
            inner.snapshot.events.push(event);
            trim_vec(&mut inner.snapshot.events, EVENT_HISTORY_LIMIT);
        }
        let _ = self.events.send(emitted_event);
    }

    fn trim_tunnel_history(&self, keep_id: &str) {
        let mut inner = self.inner.lock().expect("host state lock poisoned");
        if inner.snapshot.tunnels.len() <= TUNNEL_HISTORY_LIMIT {
            return;
        }
        let mut index = 0;
        while inner.snapshot.tunnels.len() > TUNNEL_HISTORY_LIMIT
            && index < inner.snapshot.tunnels.len()
        {
            if inner.snapshot.tunnels[index].tunnel_id == keep_id {
                index += 1;
                continue;
            }
            inner.snapshot.tunnels.remove(index);
        }
        rebuild_tunnel_positions(&mut inner);
    }
}

fn upsert_command(inner: &mut HostStateInner, command: HostCommandTaskSnapshot) {
    if let Some(index) = inner.command_positions.get(&command.request_id).copied() {
        inner.snapshot.commands[index] = command;
    } else {
        let request_id = command.request_id.clone();
        inner.snapshot.commands.push(command);
        inner
            .command_positions
            .insert(request_id, inner.snapshot.commands.len() - 1);
        trim_commands(inner);
    }
}

fn find_command_mut<'a>(
    inner: &'a mut HostStateInner,
    request_id: &str,
) -> Option<&'a mut HostCommandTaskSnapshot> {
    inner
        .command_positions
        .get(request_id)
        .and_then(|index| inner.snapshot.commands.get_mut(*index))
}

fn trim_commands(inner: &mut HostStateInner) {
    if inner.snapshot.commands.len() <= TASK_HISTORY_LIMIT {
        return;
    }
    let remove_count = inner.snapshot.commands.len() - TASK_HISTORY_LIMIT;
    inner.snapshot.commands.drain(0..remove_count);
    rebuild_command_positions(inner);
}

fn rebuild_command_positions(inner: &mut HostStateInner) {
    inner.command_positions.clear();
    for (index, command) in inner.snapshot.commands.iter().enumerate() {
        inner
            .command_positions
            .insert(command.request_id.clone(), index);
    }
}

fn upsert_transfer(inner: &mut HostStateInner, transfer: HostTransferTaskSnapshot) {
    if let Some(index) = inner.transfer_positions.get(&transfer.request_id).copied() {
        inner.snapshot.transfers[index] = transfer;
    } else {
        let request_id = transfer.request_id.clone();
        inner.snapshot.transfers.push(transfer);
        inner
            .transfer_positions
            .insert(request_id, inner.snapshot.transfers.len() - 1);
        trim_transfers(inner);
    }
}

fn find_transfer_mut<'a>(
    inner: &'a mut HostStateInner,
    request_id: &str,
) -> Option<&'a mut HostTransferTaskSnapshot> {
    inner
        .transfer_positions
        .get(request_id)
        .and_then(|index| inner.snapshot.transfers.get_mut(*index))
}

fn trim_transfers(inner: &mut HostStateInner) {
    if inner.snapshot.transfers.len() <= TASK_HISTORY_LIMIT {
        return;
    }
    let remove_count = inner.snapshot.transfers.len() - TASK_HISTORY_LIMIT;
    inner.snapshot.transfers.drain(0..remove_count);
    rebuild_transfer_positions(inner);
}

fn rebuild_transfer_positions(inner: &mut HostStateInner) {
    inner.transfer_positions.clear();
    for (index, transfer) in inner.snapshot.transfers.iter().enumerate() {
        inner
            .transfer_positions
            .insert(transfer.request_id.clone(), index);
    }
}

fn upsert_tunnel(inner: &mut HostStateInner, tunnel: HostTunnelSnapshot) {
    if let Some(index) = inner.tunnel_positions.get(&tunnel.tunnel_id).copied() {
        inner.snapshot.tunnels[index] = tunnel;
    } else {
        let tunnel_id = tunnel.tunnel_id.clone();
        inner.snapshot.tunnels.push(tunnel);
        inner
            .tunnel_positions
            .insert(tunnel_id, inner.snapshot.tunnels.len() - 1);
    }
}

fn find_tunnel_mut<'a>(
    inner: &'a mut HostStateInner,
    tunnel_id: &str,
) -> Option<&'a mut HostTunnelSnapshot> {
    inner
        .tunnel_positions
        .get(tunnel_id)
        .and_then(|index| inner.snapshot.tunnels.get_mut(*index))
}

fn rebuild_tunnel_positions(inner: &mut HostStateInner) {
    inner.tunnel_positions.clear();
    for (index, tunnel) in inner.snapshot.tunnels.iter().enumerate() {
        inner
            .tunnel_positions
            .insert(tunnel.tunnel_id.clone(), index);
    }
}

fn mark_session_tasks_closed(
    snapshot: &mut HostSnapshot,
    session_id: &str,
    at: &str,
    result: &str,
) {
    for command in &mut snapshot.commands {
        if command.session_id.as_deref() == Some(session_id)
            && command.status == HostTaskStatus::Running
        {
            command.status = HostTaskStatus::Cancelled;
            command.finished_at = Some(at.to_owned());
            command.result = Some(result.to_owned());
        }
    }
    for transfer in &mut snapshot.transfers {
        if transfer.session_id.as_deref() == Some(session_id)
            && transfer.status == HostTaskStatus::Running
        {
            transfer.status = HostTaskStatus::Cancelled;
            transfer.finished_at = Some(at.to_owned());
            transfer.result = Some(result.to_owned());
        }
    }
    for tunnel in &mut snapshot.tunnels {
        if tunnel.session_id.as_deref() == Some(session_id) && tunnel.status == TunnelStatus::Active
        {
            tunnel.status = TunnelStatus::Closed;
            tunnel.last_activity_at = at.to_owned();
            tunnel.close_reason = Some(result.to_owned());
        }
    }
}

fn push_error(snapshot: &mut HostSnapshot, error: HostErrorSnapshot) {
    snapshot.recent_errors.push(error);
    trim_vec(&mut snapshot.recent_errors, RECENT_ERROR_LIMIT);
}

fn trim_vec<T>(items: &mut Vec<T>, limit: usize) {
    if items.len() > limit {
        items.drain(0..items.len() - limit);
    }
}

fn compact_label(label: String) -> String {
    const MAX_LABEL_LEN: usize = 128;
    if label.chars().count() <= MAX_LABEL_LEN {
        return label;
    }
    let mut compact = label.chars().take(MAX_LABEL_LEN).collect::<String>();
    compact.push_str("...");
    compact
}

fn redact_reason(reason: &str) -> String {
    const MAX_REASON_LEN: usize = 256;
    let mut compact = reason.replace('\n', " ");
    if compact.chars().count() > MAX_REASON_LEN {
        compact = compact.chars().take(MAX_REASON_LEN).collect::<String>();
        compact.push_str("...");
    }
    compact
}

fn command_summary(command: &str) -> String {
    format!("command={command}")
}

fn transfer_summary(direction: HostTransferDirection, size: Option<u64>) -> String {
    let direction = match direction {
        HostTransferDirection::Upload => "upload",
        HostTransferDirection::Download => "download",
    };
    match size {
        Some(size) => format!("{direction} size={size}"),
        None => direction.to_owned(),
    }
}

pub(crate) fn task_status_for_result(ok: bool, error: Option<&anyhow::Error>) -> HostTaskStatus {
    if ok {
        return HostTaskStatus::Completed;
    }
    let is_cancelled = error
        .into_iter()
        .flat_map(|err| err.chain())
        .any(|cause| cause.to_string().contains("command cancelled"));
    if is_cancelled {
        HostTaskStatus::Cancelled
    } else {
        HostTaskStatus::Failed
    }
}

fn task_status_str(status: HostTaskStatus) -> &'static str {
    match status {
        HostTaskStatus::Running => "running",
        HostTaskStatus::Completed => "completed",
        HostTaskStatus::Failed => "failed",
        HostTaskStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> HostStateHandle {
        HostStateHandle::new(HostStateMetadata {
            server_url: "http://127.0.0.1:3000".to_owned(),
            machine_id: "machine".to_owned(),
            host_id: "host".to_owned(),
            totp_period_seconds: 120,
            audit_path: PathBuf::from("audit.jsonl"),
            power: HostPowerSnapshot {
                active: true,
                warning: None,
            },
        })
    }

    fn totp() -> HostTotpSnapshot {
        HostTotpSnapshot {
            current_code: "123456".to_owned(),
            period_seconds: 120,
            remaining_seconds: 42,
        }
    }

    #[test]
    fn initial_snapshot_contains_static_host_fields() {
        let state = state();

        let snapshot = state.snapshot(totp());

        assert_eq!(snapshot.listener.status, HostListenerStatus::Stopped);
        assert_eq!(snapshot.server_url, "http://127.0.0.1:3000");
        assert_eq!(snapshot.machine_id, "machine");
        assert_eq!(snapshot.host_id, "host");
        assert_eq!(snapshot.totp.current_code, "123456");
        assert_eq!(snapshot.totp.remaining_seconds, 42);
        assert!(snapshot.commands.is_empty());
    }

    #[test]
    fn subscriber_receives_connection_event() {
        let state = state();
        let mut events = state.subscribe();

        state.record_connected();

        let event = events.try_recv().unwrap();
        assert_eq!(event.kind, HostEventKind::HostConnected);
        assert_eq!(event.status.as_deref(), Some("connected"));
        assert_eq!(
            state.snapshot(totp()).listener.status,
            HostListenerStatus::Connected
        );
    }

    #[test]
    fn session_open_close_updates_snapshot() {
        let state = state();

        state.record_session_opened("sess".to_owned(), "controller".to_owned());
        let snapshot = state.snapshot(totp());
        assert_eq!(snapshot.session.active_session_id.as_deref(), Some("sess"));
        assert_eq!(
            snapshot.session.controller_label.as_deref(),
            Some("controller")
        );

        state.record_session_closed("sess".to_owned(), "controller_close".to_owned());
        let snapshot = state.snapshot(totp());
        assert_eq!(snapshot.session.active_session_id, None);
        assert_eq!(
            snapshot.session.last_close_reason.as_deref(),
            Some("controller_close")
        );
    }

    #[test]
    fn command_lifecycle_updates_snapshot_and_events() {
        let state = state();
        let mut events = state.subscribe();

        state.record_command_started("req".to_owned(), Some("sess".to_owned()), "exec".to_owned());
        state.record_command_completed(
            "req".to_owned(),
            Some("sess".to_owned()),
            "exec".to_owned(),
            HostTaskStatus::Completed,
            12,
            "ok".to_owned(),
        );

        let started = events.try_recv().unwrap();
        let completed = events.try_recv().unwrap();
        assert_eq!(started.kind, HostEventKind::CommandStarted);
        assert_eq!(completed.kind, HostEventKind::CommandCompleted);

        let snapshot = state.snapshot(totp());
        assert_eq!(snapshot.commands.len(), 1);
        assert_eq!(snapshot.commands[0].request_id, "req");
        assert_eq!(snapshot.commands[0].status, HostTaskStatus::Completed);
        assert_eq!(snapshot.commands[0].duration_ms, Some(12));
        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.events[0].kind, HostEventKind::CommandStarted);
        assert_eq!(snapshot.events[1].kind, HostEventKind::CommandCompleted);
    }

    #[test]
    fn transfer_progress_updates_transfer_index() {
        let state = state();

        state.record_transfer_started(
            "up".to_owned(),
            Some("sess".to_owned()),
            HostTransferDirection::Upload,
            Some(10),
        );
        state.record_transfer_progress("up".to_owned(), Some("sess".to_owned()), 5);
        state.record_transfer_completed(
            "up".to_owned(),
            Some("sess".to_owned()),
            HostTaskStatus::Completed,
            Some(10),
            "ok".to_owned(),
        );

        let snapshot = state.snapshot(totp());
        assert_eq!(snapshot.transfers.len(), 1);
        assert_eq!(snapshot.transfers[0].bytes_transferred, 10);
        assert_eq!(snapshot.transfers[0].status, HostTaskStatus::Completed);
    }
}
