use std::{fs, io::Read, path::Path, time::Duration};

use anyhow::{bail, Context, Result};
use rcw_common::{
    protocol::{
        CommandStatusResultPayload, CommandTaskStatus, ExecArgs, ScreenshotArgs, UploadArgs,
        WindowInfo, COMMAND_SCREENSHOT, COMMAND_WINDOWS,
    },
    transfer::{
        commit_temp_output_file, create_temp_output_file, sha256_file, temp_output_path,
        Sha256Accumulator, CHUNK_SIZE,
    },
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    cancel::{bail_if_cancelled, CancelFlag},
    cli::Cli,
    controller_config::{config_wait_timeout, parse_duration, ControllerConfig},
    defaults::{DEFAULT_EXEC_TIMEOUT_MS, DEFAULT_EXEC_WAIT_MS, EXEC_STATUS_POLL_INTERVAL},
    output::{print_json, write_output_file},
    session::{FileSessionStore, SessionFile, SessionStore},
    transport::{ControlClient, OpenSessionRequest},
};

pub(crate) type RemoteStartHook = Box<dyn FnOnce() + Send + 'static>;
const DEFAULT_CLI_EXEC_WAIT_TIMEOUT: Duration = Duration::from_millis(DEFAULT_EXEC_WAIT_MS);

pub(crate) struct ExecCommandOptions {
    pub(crate) cwd: Option<String>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) response_wait: Option<Duration>,
    pub(crate) cancel: Option<CancelFlag>,
    pub(crate) on_remote_start: Option<RemoteStartHook>,
}

pub(crate) struct UploadPathOptions<'a> {
    pub(crate) local: &'a Path,
    pub(crate) remote: &'a str,
    pub(crate) overwrite: bool,
    pub(crate) expected_sha256: Option<String>,
    pub(crate) cancel: Option<CancelFlag>,
    pub(crate) on_remote_start: Option<RemoteStartHook>,
}

pub(crate) struct DownloadFileOptions<'a> {
    pub(crate) remote: &'a str,
    pub(crate) local: &'a Path,
    pub(crate) overwrite: bool,
    pub(crate) cancel: Option<CancelFlag>,
    pub(crate) on_remote_start: Option<RemoteStartHook>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct OpenSessionResult {
    pub(crate) ok: bool,
    pub(crate) session_id: String,
    pub(crate) machine_id: String,
    pub(crate) host_id: String,
    pub(crate) server: String,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct StatusResult {
    pub(crate) ok: bool,
    pub(crate) machine_id: String,
    pub(crate) host_online: bool,
    pub(crate) session_active: bool,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct CloseResult {
    pub(crate) ok: bool,
    pub(crate) session_id: String,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct ExecResult {
    pub(crate) ok: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) duration_ms: u64,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct UploadResult {
    pub(crate) ok: bool,
    pub(crate) remote: String,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) request_id: String,
}

#[derive(Debug)]
pub(crate) struct DownloadResult {
    pub(crate) ok: bool,
    pub(crate) remote: String,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) request_id: String,
}

#[derive(Debug)]
pub(crate) struct ScreenshotResult {
    pub(crate) ok: bool,
    pub(crate) format: String,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) data: Vec<u8>,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ScreenshotFileResult {
    pub(crate) ok: bool,
    pub(crate) output_path: String,
    pub(crate) format: String,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct WindowsResult {
    pub(crate) ok: bool,
    pub(crate) windows: Value,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SimpleResult {
    pub(crate) ok: bool,
    pub(crate) summary: Option<String>,
    pub(crate) request_id: String,
}

pub(crate) async fn open_session(
    cli: &Cli,
    store: &dyn SessionStore,
    request: OpenSessionRequest<'_>,
) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let request_id = request.request_id.to_owned();
    let result = open_session_state(&config, store, request).await?;

    if cli.json {
        print_json(serde_json::to_value(&result)?)?;
    } else {
        println!(
            "opened session {} for {} ({})",
            result.session_id, result.machine_id, result.server
        );
        println!("host_id: {}", result.host_id);
        println!("request_id: {}", request_id);
    }
    Ok(0)
}

pub(crate) async fn open_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request: OpenSessionRequest<'_>,
) -> Result<OpenSessionResult> {
    let request_id = request.request_id.to_owned();
    let opened = ControlClient::new(config, store)
        .open_session(request, config_wait_timeout(config)?)
        .await?;
    let server = opened.server.clone();
    let session_id = opened.session_id.clone();
    let opened_machine_id = opened.machine_id.clone();
    let session_token = opened.session_token;
    let now = rcw_common::audit::now_rfc3339();
    let session = SessionFile {
        server: server.clone(),
        machine_id: opened_machine_id.clone(),
        session_id: session_id.clone(),
        session_token,
        created_at: now.clone(),
        last_used_at: now,
    };
    store.write_session(&session)?;

    Ok(OpenSessionResult {
        ok: true,
        session_id,
        machine_id: opened_machine_id,
        host_id: opened.host_id,
        server,
        request_id,
    })
}

pub(crate) async fn status_session(cli: &Cli, request_id: &str) -> Result<i32> {
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

pub(crate) async fn status_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
) -> Result<StatusResult> {
    let result = ControlClient::new(config, store)
        .status(request_id, config_wait_timeout(config)?)
        .await?;

    Ok(StatusResult {
        ok: result.ok,
        machine_id: result.machine_id,
        host_online: result.host_online,
        session_active: result.session_active,
        request_id: request_id.to_owned(),
    })
}

pub(crate) async fn close_session(cli: &Cli, request_id: &str) -> Result<i32> {
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

pub(crate) async fn close_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
) -> Result<CloseResult> {
    let result = ControlClient::new(config, store)
        .close_session(request_id, config_wait_timeout(config)?)
        .await?;

    Ok(CloseResult {
        ok: result.ok,
        session_id: result.session_id,
        request_id: request_id.to_owned(),
    })
}

pub(crate) async fn exec_command(
    cli: &Cli,
    request_id: &str,
    command: &[String],
    timeout: Option<&str>,
    wait: Option<&str>,
) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let remote_timeout_ms = match timeout {
        Some(value) => Some(duration_to_millis(parse_duration(value)?)),
        None => Some(DEFAULT_EXEC_TIMEOUT_MS),
    };
    let response_wait = Some(cli_exec_wait(wait)?);
    let snapshot = exec_command_state(
        &config,
        &store,
        request_id,
        command,
        ExecCommandOptions {
            cwd: None,
            timeout_ms: remote_timeout_ms,
            response_wait,
            cancel: None,
            on_remote_start: None,
        },
    )
    .await?;

    if cli.json {
        print_json(serde_json::to_value(&snapshot)?)?;
    } else {
        print_exec_snapshot_text(&snapshot, request_id)?;
    }
    Ok(exit_code_for_exec_snapshot(&snapshot))
}

pub(crate) async fn exec_status(cli: &Cli, _request_id: &str, task_id: &str) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let snapshot = ControlClient::new(&config, &store)
        .command_status(task_id)
        .await?;
    if cli.json {
        print_json(serde_json::to_value(&snapshot)?)?;
        return Ok(exit_code_for_exec_snapshot(&snapshot));
    }
    match snapshot.status {
        CommandTaskStatus::Running
        | CommandTaskStatus::Completed
        | CommandTaskStatus::Failed
        | CommandTaskStatus::Cancelled => print_exec_snapshot_text(&snapshot, task_id)?,
    }
    Ok(exit_code_for_exec_snapshot(&snapshot))
}

pub(crate) async fn exec_cancel(cli: &Cli, _request_id: &str, task_id: &str) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    ControlClient::new(&config, &store)
        .cancel_command(task_id)
        .await?;
    let snapshot = ControlClient::new(&config, &store)
        .command_status(task_id)
        .await?;
    if cli.json {
        print_json(serde_json::to_value(&snapshot)?)?;
    } else {
        println!("requested cancel for exec task {task_id}");
        println!("status: {}", command_status_label(snapshot.status));
        println!("request_id: {}", snapshot.request_id);
    }
    Ok(exit_code_for_exec_snapshot(&snapshot))
}

pub(crate) async fn start_exec_job_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &[String],
    cwd: Option<String>,
    timeout_ms: Option<u64>,
) -> Result<CommandStatusResultPayload> {
    if command.is_empty() {
        bail!("exec requires a program");
    }
    let args = json!(ExecArgs {
        program: command[0].clone(),
        argv: command[1..].to_vec(),
        cwd,
        timeout_ms,
    });
    ControlClient::new(config, store)
        .start_exec_job(request_id, args)
        .await
}

pub(crate) async fn exec_command_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &[String],
    options: ExecCommandOptions,
) -> Result<CommandStatusResultPayload> {
    let ExecCommandOptions {
        cwd,
        timeout_ms,
        response_wait,
        cancel,
        on_remote_start,
    } = options;
    let client = ControlClient::new(config, store);
    start_exec_job_state(config, store, request_id, command, cwd, timeout_ms).await?;
    if let Some(on_remote_start) = on_remote_start {
        on_remote_start();
    }
    let snapshot = match response_wait {
        Some(wait) => wait_for_exec_job_completion(&client, request_id, wait, cancel).await?,
        None => wait_for_exec_job_completion_unbounded(&client, request_id, cancel).await?,
    };
    Ok(snapshot)
}

pub(crate) fn duration_to_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn cli_exec_wait(wait: Option<&str>) -> Result<Duration> {
    match wait {
        Some(value) => parse_duration(value),
        None => Ok(DEFAULT_CLI_EXEC_WAIT_TIMEOUT),
    }
}

async fn wait_for_exec_job_completion(
    client: &ControlClient<'_>,
    task_id: &str,
    wait: Duration,
    cancel: Option<CancelFlag>,
) -> Result<CommandStatusResultPayload> {
    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    loop {
        bail_if_cancelled(cancel.as_ref())?;
        let snapshot = client.command_status(task_id).await?;
        if snapshot.status != CommandTaskStatus::Running {
            return Ok(snapshot);
        }
        tokio::select! {
            _ = &mut deadline => return Ok(snapshot),
            _ = tokio::time::sleep(EXEC_STATUS_POLL_INTERVAL) => {}
        }
    }
}

async fn wait_for_exec_job_completion_unbounded(
    client: &ControlClient<'_>,
    task_id: &str,
    cancel: Option<CancelFlag>,
) -> Result<CommandStatusResultPayload> {
    loop {
        bail_if_cancelled(cancel.as_ref())?;
        let snapshot = client.command_status(task_id).await?;
        if snapshot.status != CommandTaskStatus::Running {
            return Ok(snapshot);
        }
        tokio::time::sleep(EXEC_STATUS_POLL_INTERVAL).await;
    }
}

fn exec_result_from_snapshot(snapshot: CommandStatusResultPayload) -> Result<ExecResult> {
    if let Some(error) = snapshot.error {
        bail!("{:?}: {}", error.code, error.message);
    }
    let complete = snapshot.complete.context("missing command.complete")?;
    Ok(ExecResult {
        ok: complete.ok,
        exit_code: complete.exit_code,
        stdout: snapshot.stdout,
        stderr: snapshot.stderr,
        stdout_truncated: snapshot.stdout_truncated,
        stderr_truncated: snapshot.stderr_truncated,
        duration_ms: complete.duration_ms,
        request_id: snapshot.request_id,
    })
}

fn print_exec_snapshot_text(snapshot: &CommandStatusResultPayload, task_id: &str) -> Result<()> {
    match snapshot.status {
        CommandTaskStatus::Running => {
            println!("task_id: {}", snapshot.task_id);
            println!("status: {}", command_status_label(snapshot.status));
            println!("request_id: {}", snapshot.request_id);
        }
        CommandTaskStatus::Completed => {
            let result = exec_result_from_snapshot(snapshot.clone())?;
            print!("{}", result.stdout);
            eprint!("{}", result.stderr);
            eprintln!("task_id: {task_id}");
            eprintln!("request_id: {}", result.request_id);
        }
        CommandTaskStatus::Failed | CommandTaskStatus::Cancelled => {
            print!("{}", snapshot.stdout);
            eprint!("{}", snapshot.stderr);
            eprintln!("task_id: {task_id}");
            eprintln!("status: {}", command_status_label(snapshot.status));
            eprintln!("request_id: {}", snapshot.request_id);
            if let Some(error) = &snapshot.error {
                eprintln!("error: {:?}: {}", error.code, error.message);
            }
        }
    }
    Ok(())
}

fn command_status_label(status: CommandTaskStatus) -> &'static str {
    match status {
        CommandTaskStatus::Running => "running",
        CommandTaskStatus::Completed => "completed",
        CommandTaskStatus::Failed => "failed",
        CommandTaskStatus::Cancelled => "cancelled",
    }
}

fn exit_code_for_exec_snapshot(snapshot: &CommandStatusResultPayload) -> i32 {
    match snapshot.status {
        CommandTaskStatus::Running => 0,
        CommandTaskStatus::Completed => snapshot
            .complete
            .as_ref()
            .and_then(|complete| complete.exit_code)
            .unwrap_or_else(|| {
                if snapshot
                    .complete
                    .as_ref()
                    .map(|complete| complete.ok)
                    .unwrap_or(false)
                {
                    0
                } else {
                    1
                }
            }),
        CommandTaskStatus::Failed | CommandTaskStatus::Cancelled => 1,
    }
}

pub(crate) async fn upload_file(
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
        UploadPathOptions {
            local,
            remote,
            overwrite,
            expected_sha256: expected_sha256.map(ToOwned::to_owned),
            cancel: None,
            on_remote_start: None,
        },
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

pub(crate) async fn upload_path_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    options: UploadPathOptions<'_>,
) -> Result<UploadResult> {
    let UploadPathOptions {
        local,
        remote,
        overwrite,
        expected_sha256,
        cancel,
        on_remote_start,
    } = options;
    let (size, actual) = file_metadata_and_sha256(local, cancel.clone()).await?;
    if let Some(expected) = expected_sha256.as_deref() {
        if expected != actual {
            bail!("local sha256 mismatch: expected {expected}, calculated {actual}");
        }
    }
    bail_if_cancelled(cancel.as_ref())?;
    let response = ControlClient::new(config, store)
        .upload_file(
            request_id,
            local,
            UploadArgs {
                remote_path: remote.to_owned(),
                overwrite,
                sha256: actual.clone(),
                size,
            },
            config_wait_timeout(config)?,
            cancel.clone(),
            on_remote_start,
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

async fn file_metadata_and_sha256(
    path: &Path,
    cancel: Option<CancelFlag>,
) -> Result<(u64, String)> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let size = fs::metadata(&path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .len();
        bail_if_cancelled(cancel.as_ref())?;
        let mut file =
            fs::File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
        let mut hasher = Sha256Accumulator::new();
        let mut buffer = vec![0_u8; CHUNK_SIZE];
        loop {
            bail_if_cancelled(cancel.as_ref())?;
            let read = file
                .read(&mut buffer)
                .with_context(|| format!("failed to read {}", path.display()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let sha256 = hasher.finalize();
        Ok((size, sha256))
    })
    .await
    .context("failed to join file hashing task")?
}

pub(crate) async fn download_file(
    cli: &Cli,
    request_id: &str,
    remote: &str,
    local: &Path,
) -> Result<i32> {
    let config = ControllerConfig::from_cli(cli);
    let store = FileSessionStore::new(cli);
    let result = download_file_state(
        &config,
        &store,
        request_id,
        DownloadFileOptions {
            remote,
            local,
            overwrite: true,
            cancel: None,
            on_remote_start: None,
        },
    )
    .await?;

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

pub(crate) async fn download_file_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    options: DownloadFileOptions<'_>,
) -> Result<DownloadResult> {
    let DownloadFileOptions {
        remote,
        local,
        overwrite,
        cancel,
        on_remote_start,
    } = options;
    let temp_path = temp_output_path(local, request_id);
    let result = async {
        bail_if_cancelled(cancel.as_ref())?;
        let output =
            tokio::fs::File::from_std(create_temp_output_file(local, &temp_path, overwrite)?);
        let response = ControlClient::new(config, store)
            .download_to_file(
                request_id,
                remote,
                output,
                config_wait_timeout(config)?,
                cancel.clone(),
                on_remote_start,
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

pub(crate) async fn screenshot(
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

pub(crate) async fn screenshot_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    display: Option<u32>,
    format: &str,
) -> Result<ScreenshotResult> {
    let response = ControlClient::new(config, store)
        .command(
            request_id,
            COMMAND_SCREENSHOT,
            json!(ScreenshotArgs {
                display,
                format: format.to_owned(),
            }),
            config_wait_timeout(config)?,
            None,
            None,
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

pub(crate) async fn windows(cli: &Cli, request_id: &str) -> Result<i32> {
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

pub(crate) async fn windows_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
) -> Result<WindowsResult> {
    let response = ControlClient::new(config, store)
        .command(
            request_id,
            COMMAND_WINDOWS,
            json!({}),
            config_wait_timeout(config)?,
            None,
            None,
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

pub(crate) async fn simple_command(
    cli: &Cli,
    request_id: &str,
    command: &str,
    args: Value,
) -> Result<i32> {
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

pub(crate) async fn simple_command_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &str,
    args: Value,
) -> Result<SimpleResult> {
    let response = ControlClient::new(config, store)
        .command(
            request_id,
            command,
            args,
            config_wait_timeout(config)?,
            None,
            None,
        )
        .await?;
    let complete = response.complete.context("missing command.complete")?;
    Ok(SimpleResult {
        ok: complete.ok,
        summary: complete.summary,
        request_id: request_id.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::cli_exec_wait;
    use crate::defaults::DEFAULT_EXEC_TIMEOUT_MS;

    #[test]
    fn default_exec_timeout_is_24_hours() {
        assert_eq!(DEFAULT_EXEC_TIMEOUT_MS, 24 * 60 * 60 * 1000);
    }

    #[test]
    fn cli_exec_wait_defaults_to_90_seconds() {
        let wait = cli_exec_wait(None).unwrap();
        assert_eq!(wait, Duration::from_secs(90));
    }

    #[test]
    fn cli_exec_wait_accepts_zero_for_immediate_return() {
        let wait = cli_exec_wait(Some("0")).unwrap();
        assert_eq!(wait, Duration::from_secs(0));
    }

    #[test]
    fn cli_exec_wait_parses_explicit_duration() {
        let wait = cli_exec_wait(Some("5m")).unwrap();
        assert_eq!(wait, Duration::from_secs(300));
    }
}
