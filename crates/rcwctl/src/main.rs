mod audit;
mod cancel;
mod cli;
mod commands;
mod controller_config;
mod defaults;
mod jobs;
mod mcp;
mod output;
mod session;
mod transport;

use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use rcw_common::{
    ids::new_request_id,
    protocol::{
        KeyboardKeyArgs, KeyboardTypeArgs, MouseClickArgs, MouseMoveArgs, MouseScrollArgs,
        COMMAND_KEYBOARD_KEY, COMMAND_KEYBOARD_TYPE, COMMAND_MOUSE_CLICK, COMMAND_MOUSE_MOVE,
        COMMAND_MOUSE_SCROLL,
    },
};
use serde_json::json;

use crate::{
    audit::append_controller_audit,
    cli::{command_name, Cli, Commands},
    commands::{
        close_session, download_file, exec_cancel, exec_command, exec_status, open_session,
        screenshot, simple_command, status_session, upload_file, windows,
    },
    controller_config::ControllerConfig,
    mcp::run_mcp_server,
    session::FileSessionStore,
    transport::OpenSessionRequest,
};

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
            host_id,
            totp,
            totp_period_seconds,
            force,
        } => {
            open_session(
                &cli,
                &FileSessionStore::new(&cli),
                OpenSessionRequest {
                    request_id: &request_id,
                    machine_id: id,
                    host_id: host_id.as_deref(),
                    totp,
                    explicit_period: *totp_period_seconds,
                    force_reconnect: *force,
                },
            )
            .await
        }
        Commands::Status => status_session(&cli, &request_id).await,
        Commands::Exec {
            command,
            timeout,
            wait,
        } => {
            exec_command(
                &cli,
                &request_id,
                command,
                timeout.as_deref(),
                wait.as_deref(),
            )
            .await
        }
        Commands::ExecStatus { task_id } => exec_status(&cli, &request_id, task_id).await,
        Commands::ExecCancel { task_id } => exec_cancel(&cli, &request_id, task_id).await,
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
