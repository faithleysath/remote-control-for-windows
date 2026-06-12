mod audit;
mod commands;
mod connection;
mod output;
mod platform;
mod upload;

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::Parser;
use rcw_common::{config, ids::short_machine_id, totp};
use tokio::sync::watch;
use tracing::warn;

use crate::{audit::append_host_audit, connection::run_host_connection};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    totp_period_seconds: Option<u64>,
    #[arg(long)]
    audit_log: Option<PathBuf>,
}

pub(crate) struct HostContext {
    pub(crate) server_url: String,
    pub(crate) machine_id: String,
    pub(crate) totp_seed: Arc<Vec<u8>>,
    pub(crate) totp_period_seconds: u64,
    pub(crate) audit_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().compact().init();
    let args = Args::parse();
    let server_url = config::resolve_server_url(args.server.as_deref())?;
    let ws_url = config::ws_endpoint_url(&server_url, "/ws/host")?;
    let period = config::resolve_totp_period_seconds(args.totp_period_seconds)?;
    let audit_path = args.audit_log.unwrap_or_else(platform::default_audit_path);
    let material = platform::stable_machine_material()?;
    let machine_id = short_machine_id(&material);
    let seed = Arc::new(totp::random_seed());
    let power = platform::PowerGuard::acquire();

    print_startup(
        &server_url,
        &machine_id,
        period,
        power.as_ref().map(|guard| guard.active()),
    );
    let context = Arc::new(HostContext {
        server_url: server_url.clone(),
        machine_id: machine_id.clone(),
        totp_seed: seed,
        totp_period_seconds: period,
        audit_path,
    });

    update_clipboard(&context);
    tokio::spawn(totp_refresher(context.clone()));
    let (shutdown_tx, _) = watch::channel(false);

    loop {
        let shutdown_rx = shutdown_tx.subscribe();
        let mut connection = tokio::spawn(run_host_connection(
            context.clone(),
            ws_url.clone(),
            shutdown_rx,
        ));
        tokio::select! {
            result = &mut connection => {
                match result {
                    Ok(Ok(())) => println!("Connection: disconnected; reconnecting"),
                    Ok(Err(err)) => {
                        warn!("host connection failed: {err}");
                        println!("Connection: reconnecting ({err})");
                    }
                    Err(err) => {
                        warn!("host connection task failed: {err}");
                        println!("Connection: reconnecting ({err})");
                    }
                }
                append_host_audit(&context, "host.reconnecting", None, None, None, Some("retry"));
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            _ = tokio::signal::ctrl_c() => {
                println!("Connection: stopping");
                let _ = shutdown_tx.send(true);
                match tokio::time::timeout(std::time::Duration::from_secs(5), &mut connection).await {
                    Ok(Ok(Ok(()))) => println!("Connection: disconnected"),
                    Ok(Ok(Err(err))) => {
                        warn!("host connection failed: {err}");
                        println!("Connection: stop warning ({err})");
                    }
                    Ok(Err(err)) => {
                        warn!("host connection task failed: {err}");
                        println!("Connection: stop warning ({err})");
                    }
                    Err(_) => {
                        connection.abort();
                        println!("Connection: stop timed out; connection task aborted");
                    }
                }
                break;
            }
        }
    }

    drop(power);
    Ok(())
}

async fn totp_refresher(context: Arc<HostContext>) {
    loop {
        platform::sleep_until_next_totp_tick(context.totp_period_seconds).await;
        update_clipboard(&context);
    }
}

fn update_clipboard(context: &HostContext) {
    let code = totp::current_code(
        &context.totp_seed,
        context.totp_period_seconds,
        platform::unix_now(),
    )
    .unwrap_or_else(|_| "000000".to_owned());
    let text = format!(
        "远程协助连接信息\n服务器：{}\n机器 ID：{}\n验证码：{}\n验证码有效期：{} 秒\n",
        context.server_url, context.machine_id, code, context.totp_period_seconds
    );
    match platform::copy_connection_info(&text) {
        Ok(()) => println!("Clipboard: connection info copied"),
        Err(err) => println!("Clipboard: copy failed ({err}); copy ID/TOTP manually"),
    }
    println!("Machine ID: {}", context.machine_id);
    println!("Current TOTP: {code}");
}

fn print_startup(
    server_url: &str,
    machine_id: &str,
    period: u64,
    power_active: Result<bool, &anyhow::Error>,
) {
    println!("Remote Control for Windows Host");
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("Server: {server_url}");
    if platform::is_elevated() {
        println!("Privilege: ADMINISTRATOR / elevated");
    } else {
        println!("Privilege: standard user");
    }
    println!("Machine ID: {machine_id}");
    println!("TOTP period: {period}s");
    match power_active {
        Ok(true) => println!("Power: sleep/display timeout suppressed while host is running"),
        Ok(false) => println!("Power: no platform power request active"),
        Err(err) => println!("Power: warning: {err}"),
    }
    println!("Keep this window open while support is active.");
    println!("Close this window to stop remote control.");
}
