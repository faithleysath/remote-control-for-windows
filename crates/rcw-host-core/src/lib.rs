mod audit;
mod commands;
mod connection;
mod identity;
mod output;
mod platform;
mod state;
mod tunnel;
mod upload;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use rcw_common::{
    config,
    ids::{new_host_id, short_machine_id},
    totp,
};
use tokio::{sync::watch, task::JoinHandle};
use tracing::warn;

use crate::{
    audit::append_host_audit, connection::run_host_connection, identity::SingleInstanceGuard,
};

pub use state::{
    HostAuthRequestSnapshot, HostCommandTaskSnapshot, HostErrorSnapshot, HostEvent, HostEventKind,
    HostListenerSnapshot, HostListenerStatus, HostPowerSnapshot, HostSessionSnapshot, HostSnapshot,
    HostTaskStatus, HostTotpSnapshot, HostTransferDirection, HostTransferTaskSnapshot,
    HostTunnelSnapshot,
};

const RECONNECT_DELAY: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
pub struct HostConfig {
    pub server: Option<String>,
    pub totp_period_seconds: Option<u64>,
    pub audit_log: Option<PathBuf>,
}

impl HostConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_server(mut self, server: Option<String>) -> Self {
        self.server = server;
        self
    }

    pub fn with_totp_period_seconds(mut self, period: Option<u64>) -> Self {
        self.totp_period_seconds = period;
        self
    }

    pub fn with_audit_log(mut self, audit_log: Option<PathBuf>) -> Self {
        self.audit_log = audit_log;
        self
    }
}

#[derive(Clone, Debug)]
pub struct HostContext {
    pub(crate) server_url: String,
    pub(crate) host_id: String,
    pub(crate) machine_id: String,
    pub(crate) totp_seed: Arc<Vec<u8>>,
    pub(crate) totp_period_seconds: u64,
    pub(crate) audit_path: PathBuf,
    pub(crate) state: state::HostStateHandle,
}

impl HostContext {
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }

    pub fn totp_period_seconds(&self) -> u64 {
        self.totp_period_seconds
    }

    pub fn audit_path(&self) -> &std::path::Path {
        &self.audit_path
    }

    pub fn current_totp(&self) -> String {
        totp::current_code(
            &self.totp_seed,
            self.totp_period_seconds,
            platform::unix_now(),
        )
        .unwrap_or_else(|_| "000000".to_owned())
    }

    pub fn connection_info(&self) -> HostConnectionInfo {
        HostConnectionInfo {
            server_url: self.server_url.clone(),
            machine_id: self.machine_id.clone(),
            host_id: self.host_id.clone(),
            totp: self.current_totp(),
            totp_period_seconds: self.totp_period_seconds,
        }
    }

    pub fn snapshot(&self) -> HostSnapshot {
        let now = platform::unix_now();
        let remaining_seconds = self
            .totp_period_seconds
            .saturating_sub(now % self.totp_period_seconds)
            .max(1);
        self.state.snapshot(HostTotpSnapshot {
            current_code: self.current_totp(),
            period_seconds: self.totp_period_seconds,
            remaining_seconds,
        })
    }

    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<HostEvent> {
        self.state.subscribe()
    }
}

#[derive(Debug, Clone)]
pub struct HostConnectionInfo {
    pub server_url: String,
    pub machine_id: String,
    pub host_id: String,
    pub totp: String,
    pub totp_period_seconds: u64,
}

impl HostConnectionInfo {
    pub fn clipboard_text(&self) -> String {
        format!(
            "远程协助连接信息\n服务器：{}\n机器 ID：{}\nHost ID：{}\n验证码：{}\n验证码有效期：{} 秒\n",
            self.server_url,
            self.machine_id,
            self.host_id,
            self.totp,
            self.totp_period_seconds
        )
    }
}

pub struct HostService {
    _single_instance: SingleInstanceGuard,
    context: Arc<HostContext>,
    ws_url: String,
    power: Result<platform::PowerGuard>,
}

#[derive(Debug, Clone)]
pub enum HostLoopEvent {
    Reconnecting { reason: Option<String> },
    Stopping,
    Stopped,
    StopWarning { reason: String },
    StopTimedOut,
}

impl HostService {
    pub fn new(config: HostConfig) -> Result<Self> {
        platform::enable_process_dpi_awareness();

        let single_instance = SingleInstanceGuard::acquire()?;
        let server_url = config::resolve_server_url(config.server.as_deref())?;
        let ws_url = config::ws_endpoint_url(&server_url, "/ws/host")?;
        let period = config::resolve_totp_period_seconds(config.totp_period_seconds)?;
        let audit_path = config
            .audit_log
            .unwrap_or_else(platform::default_audit_path);
        let material = platform::stable_machine_material()?;
        let machine_id = short_machine_id(&material);
        let host_id = new_host_id();
        let seed = Arc::new(totp::random_seed());
        let power = platform::PowerGuard::acquire();
        let power_snapshot = match &power {
            Ok(guard) => HostPowerSnapshot {
                active: guard.active(),
                warning: None,
            },
            Err(err) => HostPowerSnapshot {
                active: false,
                warning: Some(err.to_string()),
            },
        };
        let state = state::HostStateHandle::new(state::HostStateMetadata {
            server_url: server_url.clone(),
            machine_id: machine_id.clone(),
            host_id: host_id.clone(),
            totp_period_seconds: period,
            audit_path: audit_path.clone(),
            power: power_snapshot,
        });
        let context = Arc::new(HostContext {
            server_url,
            host_id,
            machine_id,
            totp_seed: seed,
            totp_period_seconds: period,
            audit_path,
            state,
        });

        Ok(Self {
            _single_instance: single_instance,
            context,
            ws_url,
            power,
        })
    }

    pub fn context(&self) -> &Arc<HostContext> {
        &self.context
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub fn power_status(&self) -> Result<bool, &anyhow::Error> {
        self.power.as_ref().map(|guard| guard.active())
    }

    pub fn connection_info(&self) -> HostConnectionInfo {
        self.context.connection_info()
    }

    pub fn snapshot(&self) -> HostSnapshot {
        self.context.snapshot()
    }

    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<HostEvent> {
        self.context.subscribe_events()
    }

    pub fn copy_connection_info(&self) -> Result<HostConnectionInfo> {
        let info = self.connection_info();
        platform::copy_connection_info(&info.clipboard_text())?;
        Ok(info)
    }

    pub fn spawn_connection(&self, shutdown: watch::Receiver<bool>) -> JoinHandle<Result<()>> {
        let context = self.context.clone();
        let ws_url = self.ws_url.clone();
        tokio::spawn(run_host_connection(context, ws_url, shutdown))
    }

    pub async fn run_reconnect_loop<F>(&self, mut shutdown: watch::Receiver<bool>, mut on_event: F)
    where
        F: FnMut(HostLoopEvent),
    {
        'run: loop {
            if *shutdown.borrow() {
                self.context
                    .state
                    .record_listener_status(HostListenerStatus::Stopped, None);
                on_event(HostLoopEvent::Stopped);
                break;
            }

            self.context
                .state
                .record_listener_status(HostListenerStatus::Connecting, None);
            let mut connection = self.spawn_connection(shutdown.clone());
            tokio::select! {
                result = &mut connection => {
                    if *shutdown.borrow() {
                        handle_stop_result(result, &mut on_event);
                        break;
                    }

                    match result {
                        Ok(Ok(())) => {
                            self.context
                                .state
                                .record_listener_status(HostListenerStatus::Reconnecting, None);
                            on_event(HostLoopEvent::Reconnecting { reason: None });
                        }
                        Ok(Err(err)) => {
                            let reason = err.to_string();
                            warn!("host connection failed: {reason}");
                            self.context
                                .state
                                .record_listener_status(HostListenerStatus::Reconnecting, Some(reason.clone()));
                            on_event(HostLoopEvent::Reconnecting { reason: Some(reason) });
                        }
                        Err(err) => {
                            let reason = err.to_string();
                            warn!("host connection task failed: {reason}");
                            self.context
                                .state
                                .record_listener_status(HostListenerStatus::Reconnecting, Some(reason.clone()));
                            on_event(HostLoopEvent::Reconnecting { reason: Some(reason) });
                        }
                    }
                    append_host_audit(
                        self.context(),
                        "host.reconnecting",
                        None,
                        None,
                        None,
                        Some("retry"),
                    );

                    tokio::select! {
                        _ = tokio::time::sleep(RECONNECT_DELAY) => {}
                        changed = shutdown.changed() => {
                            if changed.is_ok() && *shutdown.borrow() {
                                self.context
                                    .state
                                    .record_listener_status(HostListenerStatus::Stopping, None);
                                on_event(HostLoopEvent::Stopping);
                                self.context
                                    .state
                                    .record_listener_status(HostListenerStatus::Stopped, None);
                                on_event(HostLoopEvent::Stopped);
                                break 'run;
                            }
                        }
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        self.context
                            .state
                            .record_listener_status(HostListenerStatus::Stopping, None);
                        on_event(HostLoopEvent::Stopping);
                        match tokio::time::timeout(STOP_TIMEOUT, &mut connection).await {
                            Ok(result) => handle_stop_result(result, &mut on_event),
                            Err(_) => {
                                connection.abort();
                                self.context
                                    .state
                                    .record_listener_status(HostListenerStatus::Error, Some("connection task stop timed out".to_owned()));
                                on_event(HostLoopEvent::StopTimedOut);
                            }
                        }
                        self.context
                            .state
                            .record_listener_status(HostListenerStatus::Stopped, None);
                        break;
                    }
                }
            }
        }
    }
}

pub async fn run_console_host(config: HostConfig) -> Result<()> {
    let service = HostService::new(config)?;

    print_startup(&service);
    update_clipboard(service.context());
    tokio::spawn(totp_refresher(service.context().clone()));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(true);
        }
    });
    service
        .run_reconnect_loop(shutdown_rx, print_console_event)
        .await;

    Ok(())
}

fn handle_stop_result<F>(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
    on_event: &mut F,
) where
    F: FnMut(HostLoopEvent),
{
    match result {
        Ok(Ok(())) => on_event(HostLoopEvent::Stopped),
        Ok(Err(err)) => {
            let reason = err.to_string();
            warn!("host connection failed: {reason}");
            on_event(HostLoopEvent::StopWarning { reason });
        }
        Err(err) => {
            let reason = err.to_string();
            warn!("host connection task failed: {reason}");
            on_event(HostLoopEvent::StopWarning { reason });
        }
    }
}

async fn totp_refresher(context: Arc<HostContext>) {
    loop {
        platform::sleep_until_next_totp_tick(context.totp_period_seconds).await;
        update_clipboard(&context);
    }
}

fn update_clipboard(context: &HostContext) {
    let info = context.connection_info();
    match platform::copy_connection_info(&info.clipboard_text()) {
        Ok(()) => println!("Clipboard: connection info copied"),
        Err(err) => println!("Clipboard: copy failed ({err}); copy ID/TOTP manually"),
    }
    println!("Machine ID: {}", info.machine_id);
    println!("Host ID: {}", info.host_id);
    println!("Current TOTP: {}", info.totp);
}

fn print_startup(service: &HostService) {
    let context = service.context();
    println!("Remote Control for Windows Host");
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("Server: {}", context.server_url());
    if platform::is_elevated() {
        println!("Privilege: ADMINISTRATOR / elevated");
    } else {
        println!("Privilege: standard user");
    }
    println!("Machine ID: {}", context.machine_id());
    println!("Host ID: {}", context.host_id());
    println!("TOTP period: {}s", context.totp_period_seconds());
    match service.power_status() {
        Ok(true) => println!("Power: sleep/display timeout suppressed while host is running"),
        Ok(false) => println!("Power: no platform power request active"),
        Err(err) => println!("Power: warning: {err}"),
    }
    println!("Keep this window open while support is active.");
    println!("Close this window to stop remote control.");
}

fn print_console_event(event: HostLoopEvent) {
    match event {
        HostLoopEvent::Reconnecting { reason: None } => {
            println!("Connection: disconnected; reconnecting");
        }
        HostLoopEvent::Reconnecting {
            reason: Some(reason),
        } => {
            println!("Connection: reconnecting ({reason})");
        }
        HostLoopEvent::Stopping => println!("Connection: stopping"),
        HostLoopEvent::Stopped => println!("Connection: disconnected"),
        HostLoopEvent::StopWarning { reason } => println!("Connection: stop warning ({reason})"),
        HostLoopEvent::StopTimedOut => {
            println!("Connection: stop timed out; connection task aborted");
        }
    }
}
