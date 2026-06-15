use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::RcwResult;

pub const PROTOCOL_VERSION: u16 = 5;

pub const TYPE_HOST_HELLO: &str = "host.hello";
pub const TYPE_HOST_HELLO_ACK: &str = "host.hello_ack";
pub const TYPE_HOST_AUTH_REQUEST: &str = "host.auth_request";
pub const TYPE_HOST_AUTH_RESULT: &str = "host.auth_result";
pub const TYPE_HOST_SESSION_OPENED: &str = "host.session_opened";
pub const TYPE_HOST_SESSION_CLOSED: &str = "host.session_closed";
pub const TYPE_CONTROL_OPEN: &str = "control.open";
pub const TYPE_CONTROL_OPEN_RESULT: &str = "control.open_result";
pub const TYPE_SESSION_STATUS: &str = "session.status";
pub const TYPE_SESSION_STATUS_RESULT: &str = "session.status_result";
pub const TYPE_SESSION_CLOSE: &str = "session.close";
pub const TYPE_SESSION_CLOSE_RESULT: &str = "session.close_result";
pub const TYPE_COMMAND_REQUEST: &str = "command.request";
pub const TYPE_COMMAND_OUTPUT: &str = "command.output";
pub const TYPE_COMMAND_COMPLETE: &str = "command.complete";
pub const TYPE_COMMAND_START: &str = "command.start";
pub const TYPE_COMMAND_START_RESULT: &str = "command.start_result";
pub const TYPE_COMMAND_STATUS: &str = "command.status";
pub const TYPE_COMMAND_STATUS_RESULT: &str = "command.status_result";
pub const TYPE_COMMAND_CANCEL: &str = "command.cancel";
pub const TYPE_COMMAND_CANCEL_RESULT: &str = "command.cancel_result";
pub const TYPE_UPLOAD_COMPLETE: &str = "upload.complete";
pub const TYPE_DOWNLOAD_COMPLETE: &str = "download.complete";
pub const TYPE_TUNNEL_OPEN: &str = "tunnel.open";
pub const TYPE_TUNNEL_OPEN_RESULT: &str = "tunnel.open_result";
pub const TYPE_TUNNEL_STATUS: &str = "tunnel.status";
pub const TYPE_TUNNEL_STATUS_RESULT: &str = "tunnel.status_result";
pub const TYPE_TUNNEL_CLOSE: &str = "tunnel.close";
pub const TYPE_TUNNEL_CLOSE_RESULT: &str = "tunnel.close_result";
pub const TYPE_TUNNEL_STREAM_OPEN: &str = "tunnel.stream_open";
pub const TYPE_TUNNEL_STREAM_OPEN_RESULT: &str = "tunnel.stream_open_result";
pub const TYPE_TUNNEL_STREAM_EOF: &str = "tunnel.stream_eof";
pub const TYPE_TUNNEL_STREAM_RESET: &str = "tunnel.stream_reset";
pub const TYPE_ERROR: &str = "error";

pub const COMMAND_EXEC: &str = "exec";
pub const COMMAND_UPLOAD_BEGIN: &str = "upload.begin";
pub const COMMAND_DOWNLOAD_BEGIN: &str = "download.begin";
pub const COMMAND_SCREENSHOT: &str = "screenshot";
pub const COMMAND_WINDOWS: &str = "windows";
pub const COMMAND_MOUSE_MOVE: &str = "mouse.move";
pub const COMMAND_MOUSE_CLICK: &str = "mouse.click";
pub const COMMAND_MOUSE_SCROLL: &str = "mouse.scroll";
pub const COMMAND_KEYBOARD_TYPE: &str = "keyboard.type";
pub const COMMAND_KEYBOARD_KEY: &str = "keyboard.key";
pub const DEFAULT_SCREENSHOT_FORMAT: &str = "png";
pub const DEFAULT_MOUSE_BUTTON: &str = "left";
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 1024 * 1024;
pub const DEFAULT_TUNNEL_IDLE_TIMEOUT_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

impl WireMessage {
    pub fn new<P>(
        kind: impl Into<String>,
        request_id: Option<String>,
        session_id: Option<String>,
        payload: P,
    ) -> RcwResult<Self>
    where
        P: Serialize,
    {
        Ok(Self {
            kind: kind.into(),
            request_id,
            session_id,
            payload: serde_json::to_value(payload)?,
        })
    }

    pub fn empty(
        kind: impl Into<String>,
        request_id: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            request_id,
            session_id,
            payload: json!({}),
        }
    }

    pub fn payload_as<T>(&self) -> RcwResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(serde_json::from_value(self.payload.clone())?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostHelloPayload {
    pub protocol_version: u16,
    pub host_version: String,
    pub host_id: String,
    pub machine_id: String,
    pub totp_period_seconds: u64,
    pub os: String,
    pub hostname_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostHelloAckPayload {
    pub server_time: String,
    pub heartbeat_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlOpenPayload {
    pub protocol_version: u16,
    pub control_token: String,
    pub machine_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    pub totp: String,
    pub totp_period_seconds: u64,
    #[serde(default)]
    pub force_reconnect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostAuthRequestPayload {
    pub totp: String,
    pub controller_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostAuthResultPayload {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSessionOpenedPayload {
    pub session_id: String,
    pub controller_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSessionClosedPayload {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlOpenResultPayload {
    pub ok: bool,
    pub session_id: String,
    pub session_token: String,
    pub host_id: String,
    pub machine_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatusPayload {
    pub session_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatusResultPayload {
    pub ok: bool,
    pub machine_id: String,
    pub host_online: bool,
    pub session_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClosePayload {
    pub session_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCloseResultPayload {
    pub ok: bool,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRequestPayload {
    pub session_token: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_label: Option<String>,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutputPayload {
    pub stream: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommandCompletePayload {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommandStatusPayload {
    pub session_token: String,
    pub task_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandTaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommandStatusResultPayload {
    pub task_id: String,
    pub status: CommandTaskStatus,
    pub request_id: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<CommandCompletePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommandCancelPayload {
    pub session_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommandCancelResultPayload {
    pub ok: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelDirection {
    Local,
    Remote,
}

impl TunnelDirection {
    pub fn local_endpoint_side(self) -> TunnelEndpointSide {
        match self {
            TunnelDirection::Local => TunnelEndpointSide::Controller,
            TunnelDirection::Remote => TunnelEndpointSide::Host,
        }
    }

    pub fn target_endpoint_side(self) -> TunnelEndpointSide {
        match self {
            TunnelDirection::Local => TunnelEndpointSide::Host,
            TunnelDirection::Remote => TunnelEndpointSide::Controller,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelEndpointSide {
    Controller,
    Host,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStatus {
    Opening,
    Active,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStreamStatus {
    Opening,
    Active,
    Closing,
    Closed,
    Reset,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelOpenPayload {
    pub session_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,
    pub direction: TunnelDirection,
    pub listen_addr: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
    #[serde(default = "default_tunnel_idle_timeout_ms")]
    pub idle_timeout_ms: u64,
    #[serde(default)]
    pub allow_non_loopback_listen: bool,
    #[serde(default)]
    pub allow_non_loopback_target: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelOpenResultPayload {
    pub ok: bool,
    pub tunnel: TunnelInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelStatusPayload {
    pub session_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelStatusResultPayload {
    pub ok: bool,
    pub tunnels: Vec<TunnelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelClosePayload {
    pub session_token: String,
    pub tunnel_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelCloseResultPayload {
    pub ok: bool,
    pub tunnel: TunnelInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelStreamOpenPayload {
    pub tunnel_id: String,
    pub stream_id: String,
    pub target_host: String,
    pub target_port: u16,
    pub source_side: TunnelEndpointSide,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelStreamOpenResultPayload {
    pub tunnel_id: String,
    pub stream_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelStreamControlPayload {
    pub tunnel_id: String,
    pub stream_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub session_id: String,
    pub direction: TunnelDirection,
    pub listen_addr: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub status: TunnelStatus,
    pub opened_at: String,
    pub last_activity_at: String,
    pub idle_timeout_ms: u64,
    pub bytes_from_listener: u64,
    pub bytes_from_target: u64,
    pub active_streams: usize,
    pub total_streams: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
}

pub fn default_tunnel_idle_timeout_ms() -> u64 {
    DEFAULT_TUNNEL_IDLE_TIMEOUT_MS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecArgs {
    pub program: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadArgs {
    pub remote_path: String,
    pub overwrite: bool,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadArgs {
    pub remote_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<u32>,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseMoveArgs {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseClickArgs {
    pub x: i32,
    pub y: i32,
    pub button: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseScrollArgs {
    pub delta: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardTypeArgs {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardKeyArgs {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RectInfo {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub handle: String,
    pub title: String,
    pub process_id: u32,
    pub rect: RectInfo,
    pub visible: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidToken,
    HostNotFound,
    HostBusy,
    InvalidTotp,
    InvalidTotpPeriod,
    SessionExpired,
    HostDisconnected,
    RequestTimeout,
    CommandFailed,
    UnsupportedCommand,
    InvalidPath,
    ChecksumMismatch,
    PermissionDenied,
    Cancelled,
    InternalError,
}

impl ErrorCode {
    pub fn message(self) -> &'static str {
        match self {
            ErrorCode::InvalidToken => "control token is invalid",
            ErrorCode::HostNotFound => "host is not online",
            ErrorCode::HostBusy => "host already has an active session",
            ErrorCode::InvalidTotp => "TOTP is invalid or expired",
            ErrorCode::InvalidTotpPeriod => "TOTP period does not match the host",
            ErrorCode::SessionExpired => "session is expired or invalid",
            ErrorCode::HostDisconnected => "host disconnected",
            ErrorCode::RequestTimeout => "request timed out",
            ErrorCode::CommandFailed => "command failed",
            ErrorCode::UnsupportedCommand => "command is unsupported",
            ErrorCode::InvalidPath => "path is invalid",
            ErrorCode::ChecksumMismatch => "checksum mismatch",
            ErrorCode::PermissionDenied => "permission denied",
            ErrorCode::Cancelled => "request cancelled",
            ErrorCode::InternalError => "internal error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_message_round_trips_type_field() {
        let message = WireMessage::new(
            TYPE_SESSION_STATUS,
            Some("req".to_owned()),
            Some("sess".to_owned()),
            SessionStatusPayload {
                session_token: "secret".to_owned(),
            },
        )
        .unwrap();
        let encoded = serde_json::to_string(&message).unwrap();
        assert!(encoded.contains("\"type\":\"session.status\""));

        let decoded: WireMessage = serde_json::from_str(&encoded).unwrap();
        let payload: SessionStatusPayload = decoded.payload_as().unwrap();
        assert_eq!(payload.session_token, "secret");
    }

    #[test]
    fn exec_args_accept_missing_timeout() {
        let args: ExecArgs = serde_json::from_value(json!({
            "program": "cmd.exe",
            "argv": ["/c", "echo ok"]
        }))
        .unwrap();

        assert_eq!(args.program, "cmd.exe");
        assert_eq!(args.timeout_ms, None);
    }

    #[test]
    fn protocol_version_marks_host_identity_routing() {
        assert_eq!(PROTOCOL_VERSION, 5);
    }

    #[test]
    fn command_cancel_payload_requires_session_token() {
        let missing = serde_json::from_value::<CommandCancelPayload>(json!({}));
        assert!(missing.is_err());

        let payload = serde_json::from_value::<CommandCancelPayload>(json!({
            "session_token": "secret"
        }))
        .unwrap();
        assert_eq!(payload.session_token, "secret");
    }

    #[test]
    fn command_cancel_result_round_trips() {
        let message = WireMessage::new(
            TYPE_COMMAND_CANCEL_RESULT,
            Some("req".to_owned()),
            Some("sess".to_owned()),
            CommandCancelResultPayload { ok: true },
        )
        .unwrap();

        let payload: CommandCancelResultPayload = message.payload_as().unwrap();
        assert!(payload.ok);
    }

    #[test]
    fn command_status_result_round_trips() {
        let message = WireMessage::new(
            TYPE_COMMAND_STATUS_RESULT,
            Some("status-req".to_owned()),
            Some("sess".to_owned()),
            CommandStatusResultPayload {
                task_id: "task".to_owned(),
                status: CommandTaskStatus::Running,
                request_id: "task".to_owned(),
                started_at: "now".to_owned(),
                finished_at: None,
                stdout: String::new(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                complete: None,
                error: None,
            },
        )
        .unwrap();

        let payload: CommandStatusResultPayload = message.payload_as().unwrap();
        assert_eq!(payload.task_id, "task");
        assert_eq!(payload.status, CommandTaskStatus::Running);
    }

    #[test]
    fn tunnel_open_payload_round_trips() {
        let message = WireMessage::new(
            TYPE_TUNNEL_OPEN,
            Some("req".to_owned()),
            Some("sess".to_owned()),
            TunnelOpenPayload {
                session_token: "secret".to_owned(),
                tunnel_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned()),
                direction: TunnelDirection::Local,
                listen_addr: "127.0.0.1".to_owned(),
                listen_port: 15432,
                target_host: "127.0.0.1".to_owned(),
                target_port: 5432,
                idle_timeout_ms: DEFAULT_TUNNEL_IDLE_TIMEOUT_MS,
                allow_non_loopback_listen: false,
                allow_non_loopback_target: false,
            },
        )
        .unwrap();

        let payload: TunnelOpenPayload = message.payload_as().unwrap();
        assert_eq!(payload.direction, TunnelDirection::Local);
        assert_eq!(payload.listen_port, 15432);
        assert_eq!(payload.target_port, 5432);
    }
}
