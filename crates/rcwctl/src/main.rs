use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    audit::{append_jsonl, AuditEvent},
    config,
    ids::new_request_id,
    protocol::{
        CommandCompletePayload, CommandOutputPayload, CommandRequestPayload, ControlOpenPayload,
        ControlOpenResultPayload, DownloadArgs, ErrorPayload, ExecArgs, KeyboardKeyArgs,
        KeyboardTypeArgs, MouseClickArgs, MouseMoveArgs, MouseScrollArgs, ScreenshotArgs,
        SessionClosePayload, SessionCloseResultPayload, SessionStatusPayload,
        SessionStatusResultPayload, UploadArgs, WindowInfo, WireMessage, COMMAND_DOWNLOAD_BEGIN,
        COMMAND_EXEC, COMMAND_KEYBOARD_KEY, COMMAND_KEYBOARD_TYPE, COMMAND_MOUSE_CLICK,
        COMMAND_MOUSE_MOVE, COMMAND_MOUSE_SCROLL, COMMAND_SCREENSHOT, COMMAND_UPLOAD_BEGIN,
        COMMAND_WINDOWS, PROTOCOL_VERSION, TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT,
        TYPE_COMMAND_REQUEST, TYPE_CONTROL_OPEN, TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR,
        TYPE_SESSION_CLOSE, TYPE_SESSION_STATUS, TYPE_UPLOAD_COMPLETE,
    },
    transfer::{
        commit_temp_output_file, create_temp_output_file, sha256_file, temp_output_path,
        total_sequences_for_len, BinaryFrame, BinaryKind, Sha256Accumulator, CHUNK_SIZE,
    },
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, Json, ServiceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type WsStream = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
type WsSink = futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(long, global = true)]
    server: Option<String>,
    #[arg(long, global = true)]
    token: Option<String>,
    #[arg(long, global = true)]
    session: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    timeout: Option<String>,
    #[arg(long, global = true)]
    audit_label: Option<String>,
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(name = "connect")]
    Open {
        #[arg(long = "id")]
        id: String,
        #[arg(long)]
        totp: String,
        #[arg(long)]
        totp_period_seconds: Option<u64>,
    },
    Status,
    Exec {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    Upload {
        local: PathBuf,
        remote: String,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        sha256: Option<String>,
    },
    Download {
        remote: String,
        local: PathBuf,
    },
    Screenshot {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        display: Option<u32>,
        #[arg(long, default_value = "png")]
        format: String,
    },
    Windows,
    #[command(name = "mouse-move")]
    Move {
        #[arg(long)]
        x: i32,
        #[arg(long)]
        y: i32,
    },
    #[command(name = "mouse-click")]
    Click {
        #[arg(long)]
        x: i32,
        #[arg(long)]
        y: i32,
        #[arg(long, default_value = "left")]
        button: String,
    },
    #[command(name = "mouse-scroll")]
    Scroll {
        #[arg(long)]
        delta: i32,
    },
    #[command(name = "keyboard-type")]
    Type {
        text: String,
    },
    #[command(name = "keyboard-key")]
    Key {
        key: String,
    },
    #[command(name = "disconnect")]
    Close,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionFile {
    server: String,
    machine_id: String,
    session_id: String,
    session_token: String,
    created_at: String,
    last_used_at: String,
}

#[derive(Debug, Clone)]
struct ControllerConfig {
    server: Option<String>,
    token: Option<String>,
    timeout: Option<String>,
    audit_label: Option<String>,
}

impl ControllerConfig {
    fn from_cli(cli: &Cli) -> Self {
        Self {
            server: cli.server.clone(),
            token: cli.token.clone(),
            timeout: cli.timeout.clone(),
            audit_label: cli.audit_label.clone(),
        }
    }
}

const DEFAULT_MCP_TRANSFER_WAIT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Default)]
struct CommandResponse {
    stdout: String,
    stderr: String,
    file: Vec<u8>,
    json_stream: String,
    complete: Option<CommandCompletePayload>,
}

struct DownloadStreamResponse {
    complete: CommandCompletePayload,
    bytes_written: u64,
    sha256: String,
}

struct CommandSend<'a> {
    request_id: &'a str,
    command: &'a str,
    args: Value,
    terminal_kinds: &'a [&'a str],
    wait: Duration,
}

trait SessionStore: Send + Sync {
    fn read_session(&self) -> Result<SessionFile>;
    fn write_session(&self, session: &SessionFile) -> Result<()>;
    fn touch_session(&self, session: SessionFile) -> Result<()>;
    fn remove_session(&self) -> Result<()>;
}

struct FileSessionStore<'a> {
    cli: &'a Cli,
}

impl<'a> FileSessionStore<'a> {
    fn new(cli: &'a Cli) -> Self {
        Self { cli }
    }
}

impl SessionStore for FileSessionStore<'_> {
    fn read_session(&self) -> Result<SessionFile> {
        read_session(self.cli)
    }

    fn write_session(&self, session: &SessionFile) -> Result<()> {
        write_session(self.cli, session)
    }

    fn touch_session(&self, session: SessionFile) -> Result<()> {
        touch_session(self.cli, session)
    }

    fn remove_session(&self) -> Result<()> {
        remove_session(self.cli)
    }
}

#[derive(Debug, Default, Clone)]
struct MemorySessionStore {
    session: Arc<std::sync::Mutex<Option<SessionFile>>>,
}

impl MemorySessionStore {
    fn shared(session: Arc<std::sync::Mutex<Option<SessionFile>>>) -> Self {
        Self { session }
    }
}

impl SessionStore for MemorySessionStore {
    fn read_session(&self) -> Result<SessionFile> {
        self.session
            .lock()
            .map_err(|_| anyhow!("memory session lock poisoned"))?
            .clone()
            .ok_or_else(|| anyhow!("not connected; call connect first"))
    }

    fn write_session(&self, session: &SessionFile) -> Result<()> {
        *self
            .session
            .lock()
            .map_err(|_| anyhow!("memory session lock poisoned"))? = Some(session.clone());
        Ok(())
    }

    fn touch_session(&self, mut session: SessionFile) -> Result<()> {
        session.last_used_at = rcw_common::audit::now_rfc3339();
        self.write_session(&session)
    }

    fn remove_session(&self) -> Result<()> {
        *self
            .session
            .lock()
            .map_err(|_| anyhow!("memory session lock poisoned"))? = None;
        Ok(())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct OpenSessionResult {
    ok: bool,
    session_id: String,
    machine_id: String,
    server: String,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct StatusResult {
    ok: bool,
    machine_id: String,
    host_online: bool,
    session_active: bool,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct CloseResult {
    ok: bool,
    session_id: String,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ExecResult {
    ok: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    duration_ms: u64,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct UploadResult {
    ok: bool,
    remote: String,
    size: Option<u64>,
    sha256: Option<String>,
    request_id: String,
}

#[derive(Debug)]
struct DownloadResult {
    ok: bool,
    remote: String,
    size: Option<u64>,
    sha256: Option<String>,
    request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TransferKind {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TransferTaskStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct TransferTaskResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_path: Option<String>,
    size: Option<u64>,
    sha256: Option<String>,
    request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct TransferTaskStatusResult {
    task_id: String,
    status: TransferTaskStatus,
    kind: TransferKind,
    request_id: String,
    started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<TransferTaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug)]
struct ScreenshotResult {
    ok: bool,
    format: String,
    size: Option<u64>,
    sha256: Option<String>,
    data: Vec<u8>,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ScreenshotFileResult {
    ok: bool,
    output_path: String,
    format: String,
    size: Option<u64>,
    sha256: Option<String>,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WindowsResult {
    ok: bool,
    windows: Value,
    request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct SimpleResult {
    ok: bool,
    summary: Option<String>,
    request_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ConnectParams {
    #[schemars(description = "Target machine ID displayed by rcw-host.")]
    machine_id: String,
    #[schemars(description = "Current TOTP code from the target host.")]
    totp: String,
    #[serde(default)]
    #[schemars(description = "Optional TOTP period override in seconds.")]
    totp_period_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExecParams {
    #[schemars(description = "Program to execute on the remote Windows host.")]
    program: String,
    #[serde(default)]
    #[schemars(description = "Program arguments.")]
    argv: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Optional remote working directory.")]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UploadFileParams {
    #[schemars(description = "Local path readable by this MCP server process.")]
    local_path: String,
    #[schemars(description = "Destination path on the remote Windows host.")]
    remote_path: String,
    #[serde(default)]
    #[schemars(description = "Whether an existing remote file can be overwritten.")]
    overwrite: bool,
    #[serde(default)]
    #[schemars(description = "Optional expected sha256 of the local file.")]
    sha256: Option<String>,
    #[serde(default = "default_mcp_transfer_wait_timeout_ms")]
    #[schemars(
        description = "Milliseconds to wait for completion before returning a background task_id. Use 0 to always return immediately."
    )]
    wait_timeout_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DownloadFileParams {
    #[schemars(description = "Remote file path to download.")]
    remote_path: String,
    #[schemars(description = "Local path writable by this MCP server process.")]
    local_path: String,
    #[serde(default)]
    #[schemars(description = "Whether an existing local file can be overwritten.")]
    overwrite: bool,
    #[serde(default = "default_mcp_transfer_wait_timeout_ms")]
    #[schemars(
        description = "Milliseconds to wait for completion before returning a background task_id. Use 0 to always return immediately."
    )]
    wait_timeout_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TransferStatusParams {
    #[schemars(
        description = "Task ID returned by upload or download when the transfer is still running."
    )]
    task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ScreenshotFileParams {
    #[schemars(description = "Local path writable by this MCP server process.")]
    output_path: String,
    #[serde(default)]
    #[schemars(description = "Optional display index.")]
    display: Option<u32>,
    #[serde(default = "default_screenshot_format")]
    #[schemars(description = "Screenshot format. Currently png is supported.")]
    format: String,
    #[serde(default)]
    #[schemars(description = "Whether an existing local file can be overwritten.")]
    overwrite: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MoveParams {
    x: i32,
    y: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClickParams {
    x: i32,
    y: i32,
    #[serde(default = "default_mouse_button")]
    button: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ScrollParams {
    delta: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TypeParams {
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct KeyParams {
    key: String,
}

#[derive(Debug, Clone)]
struct RcwMcpServer {
    config: ControllerConfig,
    session: Arc<std::sync::Mutex<Option<SessionFile>>>,
    transfers: Arc<std::sync::Mutex<HashMap<String, TransferTaskStatusResult>>>,
}

#[tool_router(server_handler)]
impl RcwMcpServer {
    fn new(config: ControllerConfig) -> Self {
        Self {
            config,
            session: Arc::new(std::sync::Mutex::new(None)),
            transfers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    fn store(&self) -> MemorySessionStore {
        MemorySessionStore::shared(Arc::clone(&self.session))
    }

    #[tool(
        name = "connect",
        description = "Open a remote-control session and keep it in this MCP server process memory."
    )]
    async fn connect(
        &self,
        Parameters(params): Parameters<ConnectParams>,
    ) -> Result<Json<OpenSessionResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let result = open_session_state(
            &self.config,
            &self.store(),
            &request_id,
            &params.machine_id,
            &params.totp,
            params.totp_period_seconds,
        )
        .await;
        self.audit(&request_id, "mcp.connect", &result, started.elapsed(), None);
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "disconnect",
        description = "Close the active remote-control session and remove it from this MCP server process memory."
    )]
    async fn disconnect(&self) -> Result<Json<CloseResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let result = close_session_state(&self.config, &self.store(), &request_id).await;
        self.audit(
            &request_id,
            "mcp.disconnect",
            &result,
            started.elapsed(),
            None,
        );
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "status",
        description = "Check the active remote-control session and host online status."
    )]
    async fn status(&self) -> Result<Json<StatusResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let result = status_session_state(&self.config, &self.store(), &request_id).await;
        self.audit(&request_id, "mcp.status", &result, started.elapsed(), None);
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "exec",
        description = "Run a command on the remote Windows host."
    )]
    async fn exec(
        &self,
        Parameters(params): Parameters<ExecParams>,
    ) -> Result<Json<ExecResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let mut command = Vec::with_capacity(params.argv.len() + 1);
        command.push(params.program);
        command.extend(params.argv);
        let result = exec_command_state(
            &self.config,
            &self.store(),
            &request_id,
            &command,
            params.cwd,
        )
        .await;
        self.audit(&request_id, "mcp.exec", &result, started.elapsed(), None);
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "upload",
        description = "Read a local file from the MCP server filesystem and upload it to a remote Windows path."
    )]
    async fn upload(
        &self,
        Parameters(params): Parameters<UploadFileParams>,
    ) -> Result<Json<TransferTaskStatusResult>, String> {
        let UploadFileParams {
            local_path,
            remote_path,
            overwrite,
            sha256,
            wait_timeout_ms,
        } = params;
        let request_id = new_request_id();
        let task_id = new_request_id();
        let started = Instant::now();
        let local_path = PathBuf::from(&local_path);
        let local_path_display = local_path.display().to_string();
        let started_at = rcw_common::audit::now_rfc3339();
        self.insert_transfer_task(TransferTaskStatusResult {
            task_id: task_id.clone(),
            status: TransferTaskStatus::Running,
            kind: TransferKind::Upload,
            request_id: request_id.clone(),
            started_at: started_at.clone(),
            finished_at: None,
            result: None,
            error: None,
        })
        .map_err(format_error)?;

        let config = self.config.clone();
        let store = self.store();
        let transfers = Arc::clone(&self.transfers);
        let task_id_for_task = task_id.clone();
        let request_id_for_task = request_id.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = upload_path_state(
                &config,
                &store,
                &request_id_for_task,
                &local_path,
                &remote_path,
                overwrite,
                sha256,
            )
            .await
            .map(|result| TransferTaskResult {
                ok: result.ok,
                local_path: Some(local_path_display),
                remote_path: Some(result.remote),
                size: result.size,
                sha256: result.sha256,
                request_id: result.request_id,
            });
            let snapshot = finish_transfer_snapshot(
                task_id_for_task,
                TransferKind::Upload,
                request_id_for_task,
                started_at,
                result,
            );
            set_transfer_snapshot(&transfers, snapshot.clone());
            let _ = tx.send(snapshot);
        });

        let result = self
            .wait_for_transfer_task(&task_id, rx, wait_timeout_ms)
            .await;
        self.audit(
            &request_id,
            "mcp.upload",
            &result,
            started.elapsed(),
            Some(format!("task_id={task_id}")),
        );
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "download",
        description = "Download a remote file and write it to the MCP server filesystem."
    )]
    async fn download(
        &self,
        Parameters(params): Parameters<DownloadFileParams>,
    ) -> Result<Json<TransferTaskStatusResult>, String> {
        let DownloadFileParams {
            remote_path,
            local_path,
            overwrite,
            wait_timeout_ms,
        } = params;
        let request_id = new_request_id();
        let task_id = new_request_id();
        let started = Instant::now();
        let local_path = PathBuf::from(&local_path);
        let local_path_display = local_path.display().to_string();
        let started_at = rcw_common::audit::now_rfc3339();
        self.insert_transfer_task(TransferTaskStatusResult {
            task_id: task_id.clone(),
            status: TransferTaskStatus::Running,
            kind: TransferKind::Download,
            request_id: request_id.clone(),
            started_at: started_at.clone(),
            finished_at: None,
            result: None,
            error: None,
        })
        .map_err(format_error)?;

        let config = self.config.clone();
        let store = self.store();
        let transfers = Arc::clone(&self.transfers);
        let task_id_for_task = task_id.clone();
        let request_id_for_task = request_id.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = download_file_state(
                &config,
                &store,
                &request_id_for_task,
                &remote_path,
                &local_path,
                overwrite,
            )
            .await
            .map(|result| TransferTaskResult {
                ok: result.ok,
                local_path: Some(local_path_display),
                remote_path: Some(result.remote),
                size: result.size,
                sha256: result.sha256,
                request_id: result.request_id,
            });
            let snapshot = finish_transfer_snapshot(
                task_id_for_task,
                TransferKind::Download,
                request_id_for_task,
                started_at,
                result,
            );
            set_transfer_snapshot(&transfers, snapshot.clone());
            let _ = tx.send(snapshot);
        });

        let result = self
            .wait_for_transfer_task(&task_id, rx, wait_timeout_ms)
            .await;
        self.audit(
            &request_id,
            "mcp.download",
            &result,
            started.elapsed(),
            Some(format!("task_id={task_id}")),
        );
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "transfer_status",
        description = "Get the current status or final result for a background upload/download task."
    )]
    async fn transfer_status(
        &self,
        Parameters(params): Parameters<TransferStatusParams>,
    ) -> Result<Json<TransferTaskStatusResult>, String> {
        let started = Instant::now();
        let result = self.transfer_task_snapshot(&params.task_id);
        let request_id = result
            .as_ref()
            .map(|snapshot| snapshot.request_id.as_str())
            .unwrap_or(params.task_id.as_str());
        self.audit(
            request_id,
            "mcp.transfer_status",
            &result,
            started.elapsed(),
            Some(format!("task_id={}", params.task_id)),
        );
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "screenshot",
        description = "Capture a screenshot and write it to the MCP server filesystem."
    )]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotFileParams>,
    ) -> Result<Json<ScreenshotFileResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let output_path = PathBuf::from(&params.output_path);
        let result = async {
            let result = screenshot_state(
                &self.config,
                &self.store(),
                &request_id,
                params.display,
                &params.format,
            )
            .await?;
            write_output_file_checked(&output_path, &result.data, params.overwrite)?;
            if let Some(expected) = &result.sha256 {
                let actual = sha256_file(&output_path)?;
                if &actual != expected {
                    bail!("screenshot checksum mismatch: expected {expected}, calculated {actual}");
                }
            }
            Ok(ScreenshotFileResult {
                ok: result.ok,
                output_path: output_path.display().to_string(),
                format: result.format,
                size: result.size,
                sha256: result.sha256,
                request_id: result.request_id,
            })
        }
        .await;
        self.audit(
            &request_id,
            "mcp.screenshot",
            &result,
            started.elapsed(),
            None,
        );
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "windows",
        description = "List visible and known windows on the remote host."
    )]
    async fn windows(&self) -> Result<Json<WindowsResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let result = windows_state(&self.config, &self.store(), &request_id).await;
        self.audit(&request_id, "mcp.windows", &result, started.elapsed(), None);
        result.map(Json).map_err(format_error)
    }

    #[tool(
        name = "mouse_move",
        description = "Move the remote mouse cursor to absolute coordinates."
    )]
    async fn mouse_move(
        &self,
        Parameters(params): Parameters<MoveParams>,
    ) -> Result<Json<SimpleResult>, String> {
        self.simple_tool(
            "mcp.mouse_move",
            COMMAND_MOUSE_MOVE,
            json!(MouseMoveArgs {
                x: params.x,
                y: params.y,
            }),
        )
        .await
    }

    #[tool(
        name = "mouse_click",
        description = "Click the remote mouse at absolute coordinates."
    )]
    async fn mouse_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<Json<SimpleResult>, String> {
        self.simple_tool(
            "mcp.mouse_click",
            COMMAND_MOUSE_CLICK,
            json!(MouseClickArgs {
                x: params.x,
                y: params.y,
                button: params.button,
            }),
        )
        .await
    }

    #[tool(name = "mouse_scroll", description = "Scroll the remote mouse wheel.")]
    async fn mouse_scroll(
        &self,
        Parameters(params): Parameters<ScrollParams>,
    ) -> Result<Json<SimpleResult>, String> {
        self.simple_tool(
            "mcp.mouse_scroll",
            COMMAND_MOUSE_SCROLL,
            json!(MouseScrollArgs {
                delta: params.delta,
            }),
        )
        .await
    }

    #[tool(name = "keyboard_type", description = "Type text on the remote host.")]
    async fn keyboard_type(
        &self,
        Parameters(params): Parameters<TypeParams>,
    ) -> Result<Json<SimpleResult>, String> {
        self.simple_tool(
            "mcp.keyboard_type",
            COMMAND_KEYBOARD_TYPE,
            json!(KeyboardTypeArgs { text: params.text }),
        )
        .await
    }

    #[tool(
        name = "keyboard_key",
        description = "Press a key or key chord on the remote host."
    )]
    async fn keyboard_key(
        &self,
        Parameters(params): Parameters<KeyParams>,
    ) -> Result<Json<SimpleResult>, String> {
        self.simple_tool(
            "mcp.keyboard_key",
            COMMAND_KEYBOARD_KEY,
            json!(KeyboardKeyArgs { key: params.key }),
        )
        .await
    }

    fn insert_transfer_task(&self, snapshot: TransferTaskStatusResult) -> Result<()> {
        self.transfers
            .lock()
            .map_err(|_| anyhow!("transfer task lock poisoned"))?
            .insert(snapshot.task_id.clone(), snapshot);
        Ok(())
    }

    fn transfer_task_snapshot(&self, task_id: &str) -> Result<TransferTaskStatusResult> {
        self.transfers
            .lock()
            .map_err(|_| anyhow!("transfer task lock poisoned"))?
            .get(task_id)
            .cloned()
            .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))
    }

    async fn wait_for_transfer_task(
        &self,
        task_id: &str,
        rx: oneshot::Receiver<TransferTaskStatusResult>,
        wait_timeout_ms: u64,
    ) -> Result<TransferTaskStatusResult> {
        if wait_timeout_ms == 0 {
            return self.transfer_task_snapshot(task_id);
        }
        match timeout(Duration::from_millis(wait_timeout_ms), rx).await {
            Ok(Ok(snapshot)) => {
                if snapshot.status == TransferTaskStatus::Failed {
                    bail!(
                        "{}",
                        snapshot
                            .error
                            .clone()
                            .unwrap_or_else(|| "transfer failed".to_owned())
                    );
                }
                Ok(snapshot)
            }
            Ok(Err(_)) => {
                let snapshot = self.transfer_task_snapshot(task_id)?;
                if snapshot.status == TransferTaskStatus::Failed {
                    bail!(
                        "{}",
                        snapshot
                            .error
                            .clone()
                            .unwrap_or_else(|| "transfer failed".to_owned())
                    );
                }
                Ok(snapshot)
            }
            Err(_) => self.transfer_task_snapshot(task_id),
        }
    }

    async fn simple_tool(
        &self,
        audit_command: &str,
        command: &str,
        args: Value,
    ) -> Result<Json<SimpleResult>, String> {
        let request_id = new_request_id();
        let started = Instant::now();
        let result =
            simple_command_state(&self.config, &self.store(), &request_id, command, args).await;
        self.audit(&request_id, audit_command, &result, started.elapsed(), None);
        result.map(Json).map_err(format_error)
    }

    fn audit<T, E>(
        &self,
        request_id: &str,
        command: &str,
        result: &Result<T, E>,
        duration: Duration,
        summary: Option<String>,
    ) {
        let status = if result.is_ok() { "ok" } else { "failed" };
        append_controller_audit(
            &self.config,
            request_id,
            command,
            status,
            duration.as_millis() as u64,
            summary,
        );
    }
}

fn finish_transfer_snapshot(
    task_id: String,
    kind: TransferKind,
    request_id: String,
    started_at: String,
    result: Result<TransferTaskResult>,
) -> TransferTaskStatusResult {
    match result {
        Ok(result) => TransferTaskStatusResult {
            task_id,
            status: TransferTaskStatus::Completed,
            kind,
            request_id,
            started_at,
            finished_at: Some(rcw_common::audit::now_rfc3339()),
            result: Some(result),
            error: None,
        },
        Err(error) => TransferTaskStatusResult {
            task_id,
            status: TransferTaskStatus::Failed,
            kind,
            request_id,
            started_at,
            finished_at: Some(rcw_common::audit::now_rfc3339()),
            result: None,
            error: Some(format_error(error)),
        },
    }
}

fn set_transfer_snapshot(
    transfers: &Arc<std::sync::Mutex<HashMap<String, TransferTaskStatusResult>>>,
    snapshot: TransferTaskStatusResult,
) {
    if let Ok(mut transfers) = transfers.lock() {
        transfers.insert(snapshot.task_id.clone(), snapshot);
    }
}

enum IncomingFrame {
    Text(WireMessage),
    Binary(Vec<u8>),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let code = match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("rcwctl: {err:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    let cli = Cli::parse();
    let started = Instant::now();
    let request_id = new_request_id();

    let result = match &cli.command {
        Commands::Open {
            id,
            totp,
            totp_period_seconds,
        } => {
            open_session(
                &cli,
                &FileSessionStore::new(&cli),
                &request_id,
                id,
                totp,
                *totp_period_seconds,
            )
            .await
        }
        Commands::Status => status_session(&cli, &request_id).await,
        Commands::Exec { command } => exec_command(&cli, &request_id, command).await,
        Commands::Upload {
            local,
            remote,
            overwrite,
            sha256,
        } => {
            upload_file(
                &cli,
                &request_id,
                local,
                remote,
                *overwrite,
                sha256.as_deref(),
            )
            .await
        }
        Commands::Download { remote, local } => {
            download_file(&cli, &request_id, remote, local).await
        }
        Commands::Screenshot {
            output,
            display,
            format,
        } => screenshot(&cli, &request_id, output, *display, format).await,
        Commands::Windows => windows(&cli, &request_id).await,
        Commands::Move { x, y } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_MOUSE_MOVE,
                json!(MouseMoveArgs { x: *x, y: *y }),
            )
            .await
        }
        Commands::Click { x, y, button } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_MOUSE_CLICK,
                json!(MouseClickArgs {
                    x: *x,
                    y: *y,
                    button: button.clone()
                }),
            )
            .await
        }
        Commands::Scroll { delta } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_MOUSE_SCROLL,
                json!(MouseScrollArgs { delta: *delta }),
            )
            .await
        }
        Commands::Type { text } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_KEYBOARD_TYPE,
                json!(KeyboardTypeArgs { text: text.clone() }),
            )
            .await
        }
        Commands::Key { key } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_KEYBOARD_KEY,
                json!(KeyboardKeyArgs { key: key.clone() }),
            )
            .await
        }
        Commands::Close => close_session(&cli, &request_id).await,
        Commands::Mcp => run_mcp_server(&cli).await,
    };

    let audit_result = if result.is_ok() { "ok" } else { "failed" };
    if !matches!(cli.command, Commands::Mcp) {
        append_controller_audit(
            &ControllerConfig::from_cli(&cli),
            &request_id,
            command_name(&cli.command),
            audit_result,
            started.elapsed().as_millis() as u64,
            None,
        );
    }
    result
}

async fn open_session(
    cli: &Cli,
    store: &dyn SessionStore,
    request_id: &str,
    machine_id: &str,
    totp: &str,
    explicit_period: Option<u64>,
) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let result = open_session_state(
        &config,
        store,
        request_id,
        machine_id,
        totp,
        explicit_period,
    )
    .await?;

    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        println!(
            "opened session {} for {} ({})",
            result.session_id, result.machine_id, result.server
        );
        println!("request_id: {request_id}");
    }
    Ok(0)
}

async fn open_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    machine_id: &str,
    totp: &str,
    explicit_period: Option<u64>,
) -> Result<OpenSessionResult> {
    let server = config::resolve_server_url(config.server.as_deref())?;
    let token = config::control_token(config.token.as_deref())?;
    let period = config::resolve_totp_period_seconds(explicit_period)?;
    let message = WireMessage::new(
        TYPE_CONTROL_OPEN,
        Some(request_id.to_owned()),
        None,
        ControlOpenPayload {
            protocol_version: PROTOCOL_VERSION,
            control_token: token,
            machine_id: machine_id.to_owned(),
            totp: totp.to_owned(),
            totp_period_seconds: period,
        },
    )?;
    let messages = send_and_collect(
        &server,
        message,
        &[rcw_common::protocol::TYPE_CONTROL_OPEN_RESULT],
        config_wait_timeout(config)?,
    )
    .await?;
    let result: ControlOpenResultPayload = last_payload(&messages)?;
    let now = rcw_common::audit::now_rfc3339();
    let session = SessionFile {
        server: server.clone(),
        machine_id: result.machine_id.clone(),
        session_id: result.session_id.clone(),
        session_token: result.session_token,
        created_at: now.clone(),
        last_used_at: now,
    };
    store.write_session(&session)?;

    Ok(OpenSessionResult {
        ok: true,
        session_id: result.session_id,
        machine_id: result.machine_id,
        server,
        request_id: request_id.to_owned(),
    })
}

async fn status_session(cli: &Cli, request_id: &str) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = status_session_state(&config, &store, request_id).await?;

    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        println!("machine_id: {}", result.machine_id);
        println!("host_online: {}", result.host_online);
        println!("session_active: {}", result.session_active);
        println!("request_id: {request_id}");
    }
    Ok(
        if result.ok && result.host_online && result.session_active {
            0
        } else {
            1
        },
    )
}

async fn status_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
) -> Result<StatusResult> {
    let session = store.read_session()?;
    let message = WireMessage::new(
        TYPE_SESSION_STATUS,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        SessionStatusPayload {
            session_token: session.session_token.clone(),
        },
    )?;
    let messages = send_and_collect(
        &session.server,
        message,
        &[rcw_common::protocol::TYPE_SESSION_STATUS_RESULT],
        config_wait_timeout(config)?,
    )
    .await?;
    let result: SessionStatusResultPayload = last_payload(&messages)?;
    store.touch_session(session)?;

    Ok(StatusResult {
        ok: result.ok,
        machine_id: result.machine_id,
        host_online: result.host_online,
        session_active: result.session_active,
        request_id: request_id.to_owned(),
    })
}

async fn close_session(cli: &Cli, request_id: &str) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = close_session_state(&config, &store, request_id).await?;

    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        println!("closed session {}", result.session_id);
        println!("request_id: {request_id}");
    }
    Ok(0)
}

async fn close_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
) -> Result<CloseResult> {
    let session = store.read_session()?;
    let message = WireMessage::new(
        TYPE_SESSION_CLOSE,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        SessionClosePayload {
            session_token: session.session_token.clone(),
        },
    )?;
    let messages = send_and_collect(
        &session.server,
        message,
        &[rcw_common::protocol::TYPE_SESSION_CLOSE_RESULT],
        config_wait_timeout(config)?,
    )
    .await?;
    let result: SessionCloseResultPayload = last_payload(&messages)?;
    store.remove_session()?;

    Ok(CloseResult {
        ok: result.ok,
        session_id: result.session_id,
        request_id: request_id.to_owned(),
    })
}

async fn exec_command(cli: &Cli, request_id: &str, command: &[String]) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = exec_command_state(&config, &store, request_id, command, None).await?;

    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        print!("{}", result.stdout);
        eprint!("{}", result.stderr);
        eprintln!("request_id: {request_id}");
    }
    Ok(result.exit_code.unwrap_or(if result.ok { 0 } else { 1 }))
}

async fn exec_command_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &[String],
    cwd: Option<String>,
) -> Result<ExecResult> {
    if command.is_empty() {
        bail!("exec requires a program");
    }
    let wait = config_wait_timeout(config)?;
    let remote_timeout_ms = wait.as_millis().min(u64::MAX as u128) as u64;
    let response = send_command(
        config,
        store,
        request_id,
        COMMAND_EXEC,
        json!(ExecArgs {
            program: command[0].clone(),
            argv: command[1..].to_vec(),
            cwd,
            timeout_ms: remote_timeout_ms,
        }),
        wait + Duration::from_secs(10),
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    Ok(ExecResult {
        ok: complete.ok,
        exit_code: complete.exit_code,
        stdout: response.stdout,
        stderr: response.stderr,
        duration_ms: complete.duration_ms,
        request_id: request_id.to_owned(),
    })
}

async fn upload_file(
    cli: &Cli,
    request_id: &str,
    local: &Path,
    remote: &str,
    overwrite: bool,
    expected_sha256: Option<&str>,
) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = upload_path_state(
        &config,
        &store,
        request_id,
        local,
        remote,
        overwrite,
        expected_sha256.map(ToOwned::to_owned),
    )
    .await?;

    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        println!("uploaded {} -> {remote}", local.display());
        if let Some(sha256) = &result.sha256 {
            println!("sha256: {sha256}");
        }
        println!("request_id: {request_id}");
    }
    Ok(if result.ok { 0 } else { 1 })
}

async fn upload_path_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    local: &Path,
    remote: &str,
    overwrite: bool,
    expected_sha256: Option<String>,
) -> Result<UploadResult> {
    let (size, actual) = file_metadata_and_sha256(local).await?;
    if let Some(expected) = expected_sha256.as_deref() {
        if expected != actual {
            bail!("local sha256 mismatch: expected {expected}, calculated {actual}");
        }
    }
    let response = send_command_with_upload_file(
        config,
        store,
        request_id,
        local,
        UploadArgs {
            remote_path: remote.to_owned(),
            overwrite,
            sha256: actual.clone(),
            size,
        },
        config_wait_timeout(config)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    Ok(UploadResult {
        ok: complete.ok,
        remote: remote.to_owned(),
        size: complete.size,
        sha256: complete.sha256,
        request_id: request_id.to_owned(),
    })
}

async fn file_metadata_and_sha256(path: &Path) -> Result<(u64, String)> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let size = fs::metadata(&path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .len();
        let sha256 =
            sha256_file(&path).with_context(|| format!("failed to hash {}", path.display()))?;
        Ok((size, sha256))
    })
    .await
    .context("failed to join file hashing task")?
}

async fn download_file(cli: &Cli, request_id: &str, remote: &str, local: &Path) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = download_file_state(&config, &store, request_id, remote, local, true).await?;

    if cli.json {
        print_json(json!({
            "ok": result.ok,
            "remote": remote,
            "output": local,
            "size": result.size,
            "sha256": result.sha256,
            "request_id": request_id,
        }))?;
    } else {
        println!("downloaded {remote} -> {}", local.display());
        if let Some(sha256) = result.sha256 {
            println!("sha256: {sha256}");
        }
        println!("request_id: {request_id}");
    }
    Ok(if result.ok { 0 } else { 1 })
}

async fn download_file_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    remote: &str,
    local: &Path,
    overwrite: bool,
) -> Result<DownloadResult> {
    let temp_path = temp_output_path(local, request_id);
    let result = async {
        let output =
            tokio::fs::File::from_std(create_temp_output_file(local, &temp_path, overwrite)?);
        let response = send_command_download_to_file(
            config,
            store,
            request_id,
            remote,
            output,
            config_wait_timeout(config)?,
        )
        .await?;
        if let Some(expected) = response.complete.size {
            if response.bytes_written != expected {
                bail!(
                    "download size mismatch: expected {expected}, received {}",
                    response.bytes_written
                );
            }
        }
        if let Some(expected) = &response.complete.sha256 {
            if expected != &response.sha256 {
                bail!(
                    "download checksum mismatch: expected {expected}, calculated {}",
                    response.sha256
                );
            }
        }
        commit_temp_output_file(&temp_path, local, overwrite)?;
        Ok(DownloadResult {
            ok: response.complete.ok,
            remote: remote.to_owned(),
            size: response.complete.size,
            sha256: response.complete.sha256,
            request_id: request_id.to_owned(),
        })
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

async fn screenshot(
    cli: &Cli,
    request_id: &str,
    output: &Path,
    display: Option<u32>,
    format: &str,
) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = screenshot_state(&config, &store, request_id, display, format).await?;
    write_output_file(output, &result.data)?;
    if let Some(expected) = &result.sha256 {
        let actual = sha256_file(output)?;
        if &actual != expected {
            bail!("screenshot checksum mismatch: expected {expected}, calculated {actual}");
        }
    }

    if cli.json {
        print_json(json!({
            "ok": result.ok,
            "output": output,
            "size": result.size,
            "sha256": result.sha256,
            "request_id": request_id,
        }))?;
    } else {
        println!("wrote screenshot {}", output.display());
        println!("request_id: {request_id}");
    }
    Ok(if result.ok { 0 } else { 1 })
}

async fn screenshot_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    display: Option<u32>,
    format: &str,
) -> Result<ScreenshotResult> {
    let response = send_command(
        config,
        store,
        request_id,
        COMMAND_SCREENSHOT,
        json!(ScreenshotArgs {
            display,
            format: format.to_owned(),
        }),
        config_wait_timeout(config)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    Ok(ScreenshotResult {
        ok: complete.ok,
        format: format.to_owned(),
        size: complete.size,
        sha256: complete.sha256,
        data: response.file,
        request_id: request_id.to_owned(),
    })
}

async fn windows(cli: &Cli, request_id: &str) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = windows_state(&config, &store, request_id).await?;
    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        let windows: Vec<WindowInfo> = serde_json::from_value(result.windows)?;
        for window in windows {
            println!(
                "{} pid={} visible={} focused={} title={}",
                window.handle, window.process_id, window.visible, window.focused, window.title
            );
        }
        println!("request_id: {request_id}");
    }
    Ok(if result.ok { 0 } else { 1 })
}

async fn windows_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
) -> Result<WindowsResult> {
    let response = send_command(
        config,
        store,
        request_id,
        COMMAND_WINDOWS,
        json!({}),
        config_wait_timeout(config)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    let windows: Value = serde_json::from_str(&response.json_stream)?;
    Ok(WindowsResult {
        ok: complete.ok,
        windows,
        request_id: request_id.to_owned(),
    })
}

async fn simple_command(cli: &Cli, request_id: &str, command: &str, args: Value) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = simple_command_state(&config, &store, request_id, command, args).await?;
    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        println!(
            "{}",
            result.summary.clone().unwrap_or_else(|| "ok".to_owned())
        );
        println!("request_id: {request_id}");
    }
    Ok(if result.ok { 0 } else { 1 })
}

async fn simple_command_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &str,
    args: Value,
) -> Result<SimpleResult> {
    let response = send_command(
        config,
        store,
        request_id,
        command,
        args,
        config_wait_timeout(config)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    Ok(SimpleResult {
        ok: complete.ok,
        summary: complete.summary,
        request_id: request_id.to_owned(),
    })
}

async fn send_command(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &str,
    args: Value,
    wait: Duration,
) -> Result<CommandResponse> {
    send_command_with_terminal(
        config,
        store,
        CommandSend {
            request_id,
            command,
            args,
            terminal_kinds: &[TYPE_COMMAND_COMPLETE],
            wait,
        },
    )
    .await
}

async fn send_command_with_terminal(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    send: CommandSend<'_>,
) -> Result<CommandResponse> {
    let session = store.read_session()?;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: send.command.to_owned(),
        audit_label: config.audit_label.clone(),
        args: send.args,
    };
    let message = WireMessage::new(
        TYPE_COMMAND_REQUEST,
        Some(send.request_id.to_owned()),
        Some(session.session_id.clone()),
        payload,
    )?;
    let messages =
        send_and_collect(&session.server, message, send.terminal_kinds, send.wait).await?;
    store.touch_session(session)?;
    command_response(messages)
}

async fn send_command_with_upload_file(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    local: &Path,
    args: UploadArgs,
    wait: Duration,
) -> Result<CommandResponse> {
    let session = store.read_session()?;
    let size = args.size;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: COMMAND_UPLOAD_BEGIN.to_owned(),
        audit_label: config.audit_label.clone(),
        args: json!(args),
    };
    let message = WireMessage::new(
        TYPE_COMMAND_REQUEST,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        payload,
    )?;
    let (mut sink, mut stream) = connect_control(&session.server).await?;
    send_json(&mut sink, message).await?;
    let mut messages = Vec::new();
    {
        let mut send_file = Box::pin(send_file_binary_chunks(
            &mut sink,
            request_id,
            BinaryKind::UploadChunk,
            local,
            size,
        ));
        loop {
            tokio::select! {
                result = &mut send_file => {
                    result?;
                    break;
                }
                frame = next_message_unbounded(&mut stream) => {
                    let frame = frame?;
                    let terminal = is_terminal_frame(&frame, &[TYPE_UPLOAD_COMPLETE])?;
                    messages.push(frame);
                    if terminal {
                        store.touch_session(session)?;
                        return command_response(messages);
                    }
                }
            }
        }
    }
    messages.extend(collect_until_terminal(&mut stream, &[TYPE_UPLOAD_COMPLETE], wait).await?);
    store.touch_session(session)?;
    command_response(messages)
}

async fn send_command_download_to_file(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    remote: &str,
    mut output: tokio::fs::File,
    wait: Duration,
) -> Result<DownloadStreamResponse> {
    let session = store.read_session()?;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: COMMAND_DOWNLOAD_BEGIN.to_owned(),
        audit_label: config.audit_label.clone(),
        args: json!(DownloadArgs {
            remote_path: remote.to_owned()
        }),
    };
    let message = WireMessage::new(
        TYPE_COMMAND_REQUEST,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        payload,
    )?;
    let (mut sink, mut stream) = connect_control(&session.server).await?;
    send_json(&mut sink, message).await?;

    let mut hasher = Sha256Accumulator::new();
    let mut bytes_written = 0_u64;
    loop {
        let frame = next_message(&mut stream, wait).await?;
        match frame {
            IncomingFrame::Text(message) => {
                if message.kind == TYPE_ERROR {
                    let error: ErrorPayload = message.payload_as()?;
                    bail!("{:?}: {}", error.code, error.message);
                }
                if message.kind == TYPE_DOWNLOAD_COMPLETE {
                    output.flush().await?;
                    output.sync_all().await?;
                    drop(output);
                    store.touch_session(session)?;
                    return Ok(DownloadStreamResponse {
                        complete: message.payload_as()?,
                        bytes_written,
                        sha256: hasher.finalize(),
                    });
                }
            }
            IncomingFrame::Binary(bytes) => {
                let frame = BinaryFrame::decode(&bytes)?;
                if frame.request_id != request_id {
                    bail!(
                        "download binary frame request_id mismatch: expected {request_id}, got {}",
                        frame.request_id
                    );
                }
                if frame.kind == BinaryKind::DownloadChunk {
                    output.write_all(&frame.payload).await?;
                    hasher.update(&frame.payload);
                    bytes_written += frame.payload.len() as u64;
                }
            }
        }
    }
}

async fn send_and_collect(
    server: &str,
    message: WireMessage,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<Vec<IncomingFrame>> {
    let (mut sink, mut stream) = connect_control(server).await?;
    send_json(&mut sink, message).await?;
    collect_until_terminal(&mut stream, terminal_kinds, wait).await
}

async fn collect_until_terminal(
    stream: &mut WsStream,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<Vec<IncomingFrame>> {
    let mut messages = Vec::new();
    loop {
        let frame = next_message(stream, wait).await?;
        let terminal = is_terminal_frame(&frame, terminal_kinds)?;
        messages.push(frame);
        if terminal {
            return Ok(messages);
        }
    }
}

fn is_terminal_frame(frame: &IncomingFrame, terminal_kinds: &[&str]) -> Result<bool> {
    match frame {
        IncomingFrame::Text(message) => {
            if message.kind == TYPE_ERROR {
                let error: ErrorPayload = message.payload_as()?;
                bail!("{:?}: {}", error.code, error.message);
            }
            Ok(terminal_kinds.iter().any(|kind| *kind == message.kind))
        }
        IncomingFrame::Binary(_) => Ok(false),
    }
}

async fn send_file_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    path: &Path,
    size: u64,
) -> Result<()> {
    let total_sequences = total_sequences_for_len(size)?;
    if size == 0 {
        let frame = BinaryFrame {
            kind,
            request_id: request_id.to_owned(),
            sequence: 0,
            total_sequences,
            payload: Vec::new(),
        }
        .encode()?;
        sink.send(Message::Binary(frame)).await?;
        tokio::task::yield_now().await;
        return Ok(());
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    let mut remaining = size;
    for sequence in 0..total_sequences {
        let chunk_len = remaining.min(CHUNK_SIZE as u64) as usize;
        file.read_exact(&mut buffer[..chunk_len])
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let frame = BinaryFrame {
            kind,
            request_id: request_id.to_owned(),
            sequence,
            total_sequences,
            payload: buffer[..chunk_len].to_vec(),
        }
        .encode()?;
        sink.send(Message::Binary(frame)).await?;
        tokio::task::yield_now().await;
        remaining -= chunk_len as u64;
    }
    Ok(())
}

async fn connect_control(server: &str) -> Result<(WsSink, WsStream)> {
    let url = config::ws_endpoint_url(server, "/ws/control")?;
    let (ws, _) = connect_async(url)
        .await
        .context("failed to connect to rcw-server control websocket")?;
    Ok(ws.split())
}

async fn send_json(sink: &mut WsSink, message: WireMessage) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(&message)?))
        .await?;
    Ok(())
}

async fn next_message_unbounded(stream: &mut WsStream) -> Result<IncomingFrame> {
    loop {
        let frame = stream
            .next()
            .await
            .ok_or_else(|| anyhow!("server closed control websocket"))??;
        match frame {
            Message::Text(text) => return Ok(IncomingFrame::Text(serde_json::from_str(&text)?)),
            Message::Binary(bytes) => return Ok(IncomingFrame::Binary(bytes)),
            Message::Close(_) => bail!("server closed control websocket"),
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }
}

async fn next_message(stream: &mut WsStream, wait: Duration) -> Result<IncomingFrame> {
    loop {
        let frame = timeout(wait, stream.next())
            .await
            .context("timed out waiting for server response")?
            .ok_or_else(|| anyhow!("server closed control websocket"))??;
        match frame {
            Message::Text(text) => return Ok(IncomingFrame::Text(serde_json::from_str(&text)?)),
            Message::Binary(bytes) => return Ok(IncomingFrame::Binary(bytes)),
            Message::Close(_) => bail!("server closed control websocket"),
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }
}

fn command_response(messages: Vec<IncomingFrame>) -> Result<CommandResponse> {
    let mut response = CommandResponse::default();
    for frame in messages {
        match frame {
            IncomingFrame::Text(message) => match message.kind.as_str() {
                TYPE_COMMAND_OUTPUT => {
                    let output: CommandOutputPayload = message.payload_as()?;
                    match output.stream.as_str() {
                        "stdout" => response.stdout.push_str(&output.data),
                        "stderr" => response.stderr.push_str(&output.data),
                        "json" => response.json_stream.push_str(&output.data),
                        _ => {}
                    }
                }
                TYPE_COMMAND_COMPLETE | TYPE_UPLOAD_COMPLETE | TYPE_DOWNLOAD_COMPLETE => {
                    response.complete = Some(message.payload_as()?);
                }
                _ => {}
            },
            IncomingFrame::Binary(bytes) => {
                let frame = BinaryFrame::decode(&bytes)?;
                match frame.kind {
                    BinaryKind::DownloadChunk | BinaryKind::ScreenshotChunk => {
                        response.file.extend_from_slice(&frame.payload);
                    }
                    BinaryKind::UploadChunk => {}
                }
            }
        }
    }
    Ok(response)
}

fn last_payload<T>(messages: &[IncomingFrame]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let message = messages
        .iter()
        .rev()
        .find_map(|frame| match frame {
            IncomingFrame::Text(message) => Some(message),
            IncomingFrame::Binary(_) => None,
        })
        .ok_or_else(|| anyhow!("missing response message"))?;
    Ok(message.payload_as()?)
}

fn session_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.session {
        return Ok(path.clone());
    }
    Ok(project_dirs()?.data_dir().join("session.json"))
}

fn audit_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("audit.jsonl"))
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "rcwctl").ok_or_else(|| anyhow!("failed to resolve app data dir"))
}

fn read_session(cli: &Cli) -> Result<SessionFile> {
    let path = session_path(cli)?;
    let data = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session file {}", path.display()))?;
    Ok(serde_json::from_str(&data)?)
}

fn write_session(cli: &Cli, session: &SessionFile) -> Result<()> {
    let path = session_path(cli)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(session)?;
    fs::write(&path, data)?;
    restrict_user_only(&path)?;
    Ok(())
}

fn touch_session(cli: &Cli, mut session: SessionFile) -> Result<()> {
    session.last_used_at = rcw_common::audit::now_rfc3339();
    write_session(cli, &session)
}

fn remove_session(cli: &Cli) -> Result<()> {
    let path = session_path(cli)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(unix)]
fn restrict_user_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_user_only(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_output_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn write_output_file_checked(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if !overwrite && path.exists() {
        bail!(
            "refusing to overwrite existing local file {}; set overwrite=true to replace it",
            path.display()
        );
    }
    write_output_file(path, bytes)
}

fn config_wait_timeout(config: &ControllerConfig) -> Result<Duration> {
    match &config.timeout {
        Some(value) => parse_duration(value),
        None => Ok(Duration::from_secs(30)),
    }
}

fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return Ok(Duration::from_millis(ms.parse()?));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        return Ok(Duration::from_secs(seconds.parse()?));
    }
    if let Some(minutes) = value.strip_suffix('m') {
        return Ok(Duration::from_secs(minutes.parse::<u64>()? * 60));
    }
    Ok(Duration::from_secs(value.parse()?))
}

fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn run_mcp_server(cli: &Cli) -> Result<i32> {
    let service = RcwMcpServer::new(ControllerConfig::from_cli(cli))
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(0)
}

fn format_error(error: anyhow::Error) -> String {
    format!("{error:#}")
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

fn append_controller_audit(
    config: &ControllerConfig,
    request_id: &str,
    command: &str,
    result: &str,
    duration_ms: u64,
    summary: Option<String>,
) {
    let mut event = AuditEvent::new("controller", "command.invoked");
    event.request_id = Some(request_id.to_owned());
    event.command = Some(command.to_owned());
    event.audit_label = config.audit_label.clone();
    event.result = Some(result.to_owned());
    event.duration_ms = Some(duration_ms);
    event.summary = summary;
    if let Ok(path) = audit_path() {
        let _ = append_jsonl(path, &event);
    }
}

fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Open { .. } => "connect",
        Commands::Status => "status",
        Commands::Exec { .. } => "exec",
        Commands::Upload { .. } => "upload",
        Commands::Download { .. } => "download",
        Commands::Screenshot { .. } => "screenshot",
        Commands::Windows => "windows",
        Commands::Move { .. } => "mouse.move",
        Commands::Click { .. } => "mouse.click",
        Commands::Scroll { .. } => "mouse.scroll",
        Commands::Type { .. } => "keyboard.type",
        Commands::Key { .. } => "keyboard.key",
        Commands::Close => "disconnect",
        Commands::Mcp => "mcp",
    }
}
