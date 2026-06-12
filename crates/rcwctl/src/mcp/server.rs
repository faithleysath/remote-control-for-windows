use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use super::tasks::{
    finish_transfer_snapshot, insert_transfer_task, set_transfer_snapshot, transfer_task_snapshot,
    wait_for_transfer_task, TransferTasks,
};
use super::types::{
    ClickParams, ConnectParams, DownloadFileParams, ExecParams, KeyParams, MoveParams,
    ScreenshotFileParams, ScrollParams, TransferKind, TransferStatusParams, TransferTaskResult,
    TransferTaskStatus, TransferTaskStatusResult, TypeParams, UploadFileParams,
};
use anyhow::{bail, Result};
use rcw_common::{
    ids::new_request_id,
    protocol::{
        KeyboardKeyArgs, KeyboardTypeArgs, MouseClickArgs, MouseMoveArgs, MouseScrollArgs,
        COMMAND_KEYBOARD_KEY, COMMAND_KEYBOARD_TYPE, COMMAND_MOUSE_CLICK, COMMAND_MOUSE_MOVE,
        COMMAND_MOUSE_SCROLL,
    },
    transfer::sha256_file,
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, Json, ServiceExt};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::{
    audit::append_controller_audit,
    cli::Cli,
    commands::{
        close_session_state, download_file_state, exec_command_state, open_session_state,
        screenshot_state, simple_command_state, status_session_state, upload_path_state,
        windows_state, CloseResult, ExecResult, OpenSessionResult, ScreenshotFileResult,
        SimpleResult, StatusResult, WindowsResult,
    },
    controller_config::ControllerConfig,
    output::write_output_file_checked,
    session::{MemorySessionStore, SessionFile},
};

#[derive(Debug, Clone)]
struct RcwMcpServer {
    config: ControllerConfig,
    session: Arc<std::sync::Mutex<Option<SessionFile>>>,
    transfers: TransferTasks,
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
        insert_transfer_task(
            &self.transfers,
            TransferTaskStatusResult {
                task_id: task_id.clone(),
                status: TransferTaskStatus::Running,
                kind: TransferKind::Upload,
                request_id: request_id.clone(),
                started_at: started_at.clone(),
                finished_at: None,
                result: None,
                error: None,
            },
        )
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

        let result = wait_for_transfer_task(&self.transfers, &task_id, rx, wait_timeout_ms).await;
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
        insert_transfer_task(
            &self.transfers,
            TransferTaskStatusResult {
                task_id: task_id.clone(),
                status: TransferTaskStatus::Running,
                kind: TransferKind::Download,
                request_id: request_id.clone(),
                started_at: started_at.clone(),
                finished_at: None,
                result: None,
                error: None,
            },
        )
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

        let result = wait_for_transfer_task(&self.transfers, &task_id, rx, wait_timeout_ms).await;
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
        let result = transfer_task_snapshot(&self.transfers, &params.task_id);
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

pub(crate) async fn run_mcp_server(cli: &Cli) -> Result<i32> {
    let service = RcwMcpServer::new(ControllerConfig::from_cli(cli))
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(0)
}

fn format_error(error: anyhow::Error) -> String {
    format!("{error:#}")
}
