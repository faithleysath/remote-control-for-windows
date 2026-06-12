use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) const DEFAULT_MCP_TRANSFER_WAIT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferKind {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferTaskStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TransferTaskResult {
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) local_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remote_path: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TransferTaskStatusResult {
    pub(crate) task_id: String,
    pub(crate) status: TransferTaskStatus,
    pub(crate) kind: TransferKind,
    pub(crate) request_id: String,
    pub(crate) started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<TransferTaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ConnectParams {
    #[schemars(description = "Target machine ID displayed by rcw-host.")]
    pub(super) machine_id: String,
    #[schemars(description = "Current TOTP code from the target host.")]
    pub(super) totp: String,
    #[serde(default)]
    #[schemars(description = "Optional TOTP period override in seconds.")]
    pub(super) totp_period_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ExecParams {
    #[schemars(description = "Program to execute on the remote Windows host.")]
    pub(super) program: String,
    #[serde(default)]
    #[schemars(description = "Program arguments.")]
    pub(super) argv: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Optional remote working directory.")]
    pub(super) cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UploadFileParams {
    #[schemars(description = "Local path readable by this MCP server process.")]
    pub(super) local_path: String,
    #[schemars(description = "Destination path on the remote Windows host.")]
    pub(super) remote_path: String,
    #[serde(default)]
    #[schemars(description = "Whether an existing remote file can be overwritten.")]
    pub(super) overwrite: bool,
    #[serde(default)]
    #[schemars(description = "Optional expected sha256 of the local file.")]
    pub(super) sha256: Option<String>,
    #[serde(default = "default_mcp_transfer_wait_timeout_ms")]
    #[schemars(
        description = "Milliseconds to wait for completion before returning a background task_id. Use 0 to always return immediately."
    )]
    pub(super) wait_timeout_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct DownloadFileParams {
    #[schemars(description = "Remote file path to download.")]
    pub(super) remote_path: String,
    #[schemars(description = "Local path writable by this MCP server process.")]
    pub(super) local_path: String,
    #[serde(default)]
    #[schemars(description = "Whether an existing local file can be overwritten.")]
    pub(super) overwrite: bool,
    #[serde(default = "default_mcp_transfer_wait_timeout_ms")]
    #[schemars(
        description = "Milliseconds to wait for completion before returning a background task_id. Use 0 to always return immediately."
    )]
    pub(super) wait_timeout_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TransferStatusParams {
    #[schemars(
        description = "Task ID returned by upload or download when the transfer is still running."
    )]
    pub(super) task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ScreenshotFileParams {
    #[schemars(description = "Local path writable by this MCP server process.")]
    pub(super) output_path: String,
    #[serde(default)]
    #[schemars(description = "Optional display index.")]
    pub(super) display: Option<u32>,
    #[serde(default = "default_screenshot_format")]
    #[schemars(description = "Screenshot format. Currently png is supported.")]
    pub(super) format: String,
    #[serde(default)]
    #[schemars(description = "Whether an existing local file can be overwritten.")]
    pub(super) overwrite: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct MoveParams {
    pub(super) x: i32,
    pub(super) y: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ClickParams {
    pub(super) x: i32,
    pub(super) y: i32,
    #[serde(default = "default_mouse_button")]
    pub(super) button: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ScrollParams {
    pub(super) delta: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TypeParams {
    pub(super) text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct KeyParams {
    pub(super) key: String,
}

fn default_mcp_transfer_wait_timeout_ms() -> u64 {
    DEFAULT_MCP_TRANSFER_WAIT_TIMEOUT_MS
}

fn default_screenshot_format() -> String {
    "png".to_owned()
}

fn default_mouse_button() -> String {
    "left".to_owned()
}
