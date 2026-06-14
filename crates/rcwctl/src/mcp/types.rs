use schemars::JsonSchema;
use serde::Deserialize;

use rcw_common::protocol::{DEFAULT_MOUSE_BUTTON, DEFAULT_SCREENSHOT_FORMAT};

use crate::defaults::{
    DEFAULT_EXEC_TIMEOUT_MS, DEFAULT_EXEC_WAIT_MS, DEFAULT_MCP_TRANSFER_WAIT_MS,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ConnectParams {
    #[schemars(description = "Target machine ID displayed by rcw-host.")]
    pub(super) machine_id: String,
    #[serde(default)]
    #[schemars(description = "Optional runtime Host ID displayed by rcw-host.")]
    pub(super) host_id: Option<String>,
    #[schemars(description = "Current TOTP code from the target host.")]
    pub(super) totp: String,
    #[serde(default)]
    #[schemars(description = "Optional TOTP period override in seconds.")]
    pub(super) totp_period_seconds: Option<u64>,
    #[serde(default)]
    #[schemars(
        description = "Replace an existing active session for this host after TOTP verification."
    )]
    pub(super) force_reconnect: bool,
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
    #[serde(default = "default_exec_timeout_ms")]
    #[schemars(
        description = "Maximum runtime for the remote process in milliseconds. Defaults to 24 hours. This is enforced on the Windows host and is separate from wait_ms; when it expires, the host kills the remote process tree and the exec task finishes with a timeout/error status."
    )]
    pub(super) timeout_ms: Option<u64>,
    #[serde(default = "default_mcp_exec_wait_ms")]
    #[schemars(
        description = "MCP tool call wait window in milliseconds. Defaults to 90 seconds. Use 0 to return a task_id immediately; non-zero values wait up to this many milliseconds for completion, then return the current task status if still running."
    )]
    pub(super) wait_ms: u64,
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
    #[serde(default = "default_mcp_transfer_wait_ms")]
    #[schemars(
        description = "MCP tool call wait window in milliseconds. Defaults to 60 seconds. Use 0 to return a task_id immediately; non-zero values wait up to this many milliseconds for completion, then return the current transfer status if still running."
    )]
    pub(super) wait_ms: u64,
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
    #[serde(default = "default_mcp_transfer_wait_ms")]
    #[schemars(
        description = "MCP tool call wait window in milliseconds. Defaults to 60 seconds. Use 0 to return a task_id immediately; non-zero values wait up to this many milliseconds for completion, then return the current transfer status if still running."
    )]
    pub(super) wait_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TransferStatusParams {
    #[schemars(
        description = "Task ID returned by upload or download when the transfer is still running."
    )]
    pub(super) task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TransferCancelParams {
    #[schemars(description = "Task ID returned by upload or download.")]
    pub(super) task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ExecStatusParams {
    #[schemars(description = "Task ID returned by exec when it is still running.")]
    pub(super) task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ExecCancelParams {
    #[schemars(description = "Task ID returned by exec.")]
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
    #[schemars(description = "Absolute screen X coordinate on the remote host.")]
    pub(super) x: i32,
    #[schemars(description = "Absolute screen Y coordinate on the remote host.")]
    pub(super) y: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ClickParams {
    #[schemars(description = "Absolute screen X coordinate on the remote host.")]
    pub(super) x: i32,
    #[schemars(description = "Absolute screen Y coordinate on the remote host.")]
    pub(super) y: i32,
    #[serde(default = "default_mouse_button")]
    #[schemars(description = "Mouse button to click. Supported values: left, right, or middle.")]
    pub(super) button: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ScrollParams {
    #[schemars(description = "Mouse wheel delta. Negative values scroll down.")]
    pub(super) delta: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TypeParams {
    #[schemars(description = "Text to type on the remote host.")]
    pub(super) text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct KeyParams {
    #[schemars(description = "Key or key chord to press, for example Enter or Control+C.")]
    pub(super) key: String,
}

fn default_mcp_transfer_wait_ms() -> u64 {
    DEFAULT_MCP_TRANSFER_WAIT_MS
}

fn default_mcp_exec_wait_ms() -> u64 {
    DEFAULT_EXEC_WAIT_MS
}

fn default_exec_timeout_ms() -> Option<u64> {
    Some(DEFAULT_EXEC_TIMEOUT_MS)
}

fn default_screenshot_format() -> String {
    DEFAULT_SCREENSHOT_FORMAT.to_owned()
}

fn default_mouse_button() -> String {
    DEFAULT_MOUSE_BUTTON.to_owned()
}
