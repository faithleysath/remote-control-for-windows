mod handlers;
mod state;
mod ws;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use axum::{response::IntoResponse, routing::get, Json, Router};
use rcw_common::{
    audit::{append_jsonl, AuditEvent},
    config,
    protocol::PROTOCOL_VERSION,
};
use serde_json::json;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::{
    handlers::{control_ws, host_ws},
    state::ServerState,
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) inner: Arc<ServerState>,
    pub(crate) control_token: Arc<String>,
    pub(crate) audit_path: Arc<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_filter = std::env::var("RCW_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .compact()
        .init();

    let control_token = config::control_token(None)
        .context("RCW_CONTROL_TOKEN must be set before starting rcw-server")?;
    let bind_addr: SocketAddr = config::bind_addr()
        .parse()
        .context("RCW_BIND_ADDR must be a socket address")?;
    let audit_path = PathBuf::from(config::server_audit_log_path());

    let state = AppState {
        inner: Arc::new(ServerState::new()),
        control_token: Arc::new(control_token),
        audit_path: Arc::new(audit_path),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws/host", get(host_ws))
        .route("/ws/control", get(control_ws))
        .with_state(state);

    let listener = TcpListener::bind(bind_addr).await?;
    info!("rcw-server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "rcw-server",
        "protocol_version": PROTOCOL_VERSION,
    }))
}

pub(crate) fn audit(state: &AppState, event: AuditEvent) {
    if let Err(err) = append_jsonl(state.audit_path.as_ref(), &event) {
        error!("failed to write server audit log: {err}");
    }
}
