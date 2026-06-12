use std::{fs, path::Path, time::Duration};

use anyhow::{bail, Context, Result};
use rcw_common::{
    protocol::{
        ExecArgs, ScreenshotArgs, UploadArgs, WindowInfo, COMMAND_EXEC, COMMAND_SCREENSHOT,
        COMMAND_WINDOWS,
    },
    transfer::{commit_temp_output_file, create_temp_output_file, sha256_file, temp_output_path},
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    cli::Cli,
    controller_config::{config_wait_timeout, ControllerConfig},
    output::{print_json, write_output_file},
    session::{FileSessionStore, SessionFile, SessionStore},
    transport::ControlClient,
};

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct OpenSessionResult {
    pub(crate) ok: bool,
    pub(crate) session_id: String,
    pub(crate) machine_id: String,
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

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ExecResult {
    pub(crate) ok: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
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

pub(crate) async fn open_session_state(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    machine_id: &str,
    totp: &str,
    explicit_period: Option<u64>,
) -> Result<OpenSessionResult> {
    let opened = ControlClient::new(config, store)
        .open_session(
            request_id,
            machine_id,
            totp,
            explicit_period,
            config_wait_timeout(config)?,
        )
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
        server,
        request_id: request_id.to_owned(),
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

pub(crate) async fn exec_command(cli: &Cli, request_id: &str, command: &[String]) -> Result<i32> {
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

pub(crate) async fn exec_command_state(
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
    let response = ControlClient::new(config, store)
        .command(
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

pub(crate) async fn upload_path_state(
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

pub(crate) async fn download_file(
    cli: &Cli,
    request_id: &str,
    remote: &str,
    local: &Path,
) -> Result<i32> {
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

pub(crate) async fn download_file_state(
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
        let response = ControlClient::new(config, store)
            .download_to_file(request_id, remote, output, config_wait_timeout(config)?)
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
        .command(request_id, command, args, config_wait_timeout(config)?)
        .await?;
    let complete = response.complete.context("missing command.complete")?;
    Ok(SimpleResult {
        ok: complete.ok,
        summary: complete.summary,
        request_id: request_id.to_owned(),
    })
}
