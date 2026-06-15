use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use rcw_host_core::{run_console_host, HostConfig};

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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().compact().init();
    let args = Args::parse();
    let config = HostConfig::new()
        .with_server(args.server)
        .with_totp_period_seconds(args.totp_period_seconds)
        .with_audit_log(args.audit_log);
    run_console_host(config).await
}
