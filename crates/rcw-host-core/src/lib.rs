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

use anyhow::{anyhow, Result};
use rcw_common::{
    config,
    ids::{new_host_id, short_machine_id},
    totp,
};
use tokio::{
    sync::{mpsc, oneshot, watch, Mutex},
    task::JoinHandle,
};
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
const HOST_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const HOST_CONTROL_QUEUE_CAPACITY: usize = 32;

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

#[derive(Debug, Clone)]
pub struct HostListenerControlOutcome {
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct HostSessionControlOutcome {
    pub closed: bool,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HostTaskControlOutcome {
    pub requested: bool,
    pub request_id: String,
}

#[derive(Debug, Clone)]
pub struct HostTunnelControlOutcome {
    pub closed: bool,
    pub tunnel_id: String,
}

pub(crate) type HostControlResult<T> = std::result::Result<T, String>;

pub(crate) enum HostControlRequest {
    CloseCurrentSession {
        reason: String,
        respond: oneshot::Sender<HostControlResult<HostSessionControlOutcome>>,
    },
    CancelTask {
        request_id: String,
        respond: oneshot::Sender<HostControlResult<HostTaskControlOutcome>>,
    },
    CloseTunnel {
        tunnel_id: String,
        respond: oneshot::Sender<HostControlResult<HostTunnelControlOutcome>>,
    },
}

impl HostControlRequest {
    pub(crate) fn fail(self, message: impl Into<String>) {
        let message = message.into();
        match self {
            HostControlRequest::CloseCurrentSession { respond, .. } => {
                let _ = respond.send(Err(message));
            }
            HostControlRequest::CancelTask { respond, .. } => {
                let _ = respond.send(Err(message));
            }
            HostControlRequest::CloseTunnel { respond, .. } => {
                let _ = respond.send(Err(message));
            }
        }
    }
}

struct RunningListener {
    shutdown_tx: watch::Sender<bool>,
    control_tx: mpsc::Sender<HostControlRequest>,
    join: JoinHandle<()>,
}

struct HostRuntimeParts {
    context: Arc<HostContext>,
    ws_url: String,
    power: Result<platform::PowerGuard>,
}

pub struct HostService {
    _single_instance: SingleInstanceGuard,
    context: Arc<HostContext>,
    ws_url: String,
    power: Result<platform::PowerGuard>,
    listener: Mutex<Option<RunningListener>>,
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
        let runtime = build_runtime_parts(config)?;

        Ok(Self {
            _single_instance: single_instance,
            context: runtime.context,
            ws_url: runtime.ws_url,
            power: runtime.power,
            listener: Mutex::new(None),
        })
    }

    pub async fn start_listener(&self) -> Result<HostListenerControlOutcome> {
        let mut listener = self.listener.lock().await;
        if listener.is_some() {
            return Ok(HostListenerControlOutcome { changed: false });
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (control_tx, control_rx) = mpsc::channel(HOST_CONTROL_QUEUE_CAPACITY);
        let context = self.context.clone();
        let ws_url = self.ws_url.clone();
        let join = tokio::spawn(run_controlled_reconnect_loop(
            context,
            ws_url,
            shutdown_rx,
            control_rx,
        ));
        *listener = Some(RunningListener {
            shutdown_tx,
            control_tx,
            join,
        });

        Ok(HostListenerControlOutcome { changed: true })
    }

    pub async fn stop_listener(&self) -> Result<HostListenerControlOutcome> {
        let running = self.listener.lock().await.take();
        let Some(running) = running else {
            return Ok(HostListenerControlOutcome { changed: false });
        };

        let _ = running.shutdown_tx.send(true);
        let mut join = running.join;
        if tokio::time::timeout(STOP_TIMEOUT, &mut join).await.is_err() {
            join.abort();
        }
        self.context
            .state
            .record_listener_status(HostListenerStatus::Stopped, None);
        Ok(HostListenerControlOutcome { changed: true })
    }

    pub async fn restart_listener(&self) -> Result<HostListenerControlOutcome> {
        let stopped = self.stop_listener().await?;
        let started = self.start_listener().await?;
        Ok(HostListenerControlOutcome {
            changed: stopped.changed || started.changed,
        })
    }

    pub async fn restart_with_config(
        &mut self,
        config: HostConfig,
    ) -> Result<HostListenerControlOutcome> {
        let _ = self.stop_listener().await?;
        let runtime = build_runtime_parts(config)?;
        self.context = runtime.context;
        self.ws_url = runtime.ws_url;
        self.power = runtime.power;
        self.start_listener().await
    }

    pub async fn close_current_session(&self) -> Result<HostSessionControlOutcome> {
        let (respond, receive) = oneshot::channel();
        self.send_control(HostControlRequest::CloseCurrentSession {
            reason: "host_close".to_owned(),
            respond,
        })
        .await?;
        receive_control_response(receive).await
    }

    pub async fn cancel_exec_task(
        &self,
        request_id: impl Into<String>,
    ) -> Result<HostTaskControlOutcome> {
        self.cancel_task(request_id.into()).await
    }

    pub async fn cancel_transfer_task(
        &self,
        request_id: impl Into<String>,
    ) -> Result<HostTaskControlOutcome> {
        self.cancel_task(request_id.into()).await
    }

    pub async fn close_tunnel(
        &self,
        tunnel_id: impl Into<String>,
    ) -> Result<HostTunnelControlOutcome> {
        let (respond, receive) = oneshot::channel();
        self.send_control(HostControlRequest::CloseTunnel {
            tunnel_id: tunnel_id.into(),
            respond,
        })
        .await?;
        receive_control_response(receive).await
    }

    async fn cancel_task(&self, request_id: String) -> Result<HostTaskControlOutcome> {
        let (respond, receive) = oneshot::channel();
        self.send_control(HostControlRequest::CancelTask {
            request_id,
            respond,
        })
        .await?;
        receive_control_response(receive).await
    }

    async fn send_control(&self, request: HostControlRequest) -> Result<()> {
        let control_tx = {
            let listener = self.listener.lock().await;
            listener
                .as_ref()
                .map(|listener| listener.control_tx.clone())
                .ok_or_else(|| anyhow!("host listener is not running"))?
        };
        control_tx
            .send(request)
            .await
            .map_err(|_| anyhow!("host listener control channel is closed"))
    }

    pub fn copy_connection_info_text(&self) -> String {
        self.connection_info().clipboard_text()
    }

    pub fn reconfigure_stopped(&mut self, config: HostConfig) -> Result<()> {
        let listener = self
            .listener
            .try_lock()
            .map_err(|_| anyhow!("host listener state is busy"))?;
        if listener.is_some() {
            return Err(anyhow!("host listener must be stopped before reconfigure"));
        }
        drop(listener);
        let runtime = build_runtime_parts(config)?;
        self.context = runtime.context;
        self.ws_url = runtime.ws_url;
        self.power = runtime.power;
        Ok(())
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
        tokio::spawn(run_host_connection(context, ws_url, shutdown, None))
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

fn build_runtime_parts(config: HostConfig) -> Result<HostRuntimeParts> {
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

    Ok(HostRuntimeParts {
        context,
        ws_url,
        power,
    })
}

async fn receive_control_response<T>(
    receive: oneshot::Receiver<HostControlResult<T>>,
) -> Result<T> {
    let response = tokio::time::timeout(HOST_CONTROL_TIMEOUT, receive)
        .await
        .map_err(|_| anyhow!("timed out waiting for host control response"))?
        .map_err(|_| anyhow!("host control response channel closed"))?;
    response.map_err(|message| anyhow!(message))
}

async fn run_controlled_reconnect_loop(
    context: Arc<HostContext>,
    ws_url: String,
    mut shutdown: watch::Receiver<bool>,
    mut control_rx: mpsc::Receiver<HostControlRequest>,
) {
    'run: loop {
        if *shutdown.borrow() {
            context
                .state
                .record_listener_status(HostListenerStatus::Stopped, None);
            break;
        }

        context
            .state
            .record_listener_status(HostListenerStatus::Connecting, None);
        let (conn_control_tx, conn_control_rx) = mpsc::channel(HOST_CONTROL_QUEUE_CAPACITY);
        let mut connection = tokio::spawn(run_host_connection(
            context.clone(),
            ws_url.clone(),
            shutdown.clone(),
            Some(conn_control_rx),
        ));

        loop {
            tokio::select! {
                result = &mut connection => {
                    if *shutdown.borrow() {
                        break 'run;
                    }
                    record_connection_result(&context, result);
                    break;
                }
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        context
                            .state
                            .record_listener_status(HostListenerStatus::Stopping, None);
                        match tokio::time::timeout(STOP_TIMEOUT, &mut connection).await {
                            Ok(_) => {}
                            Err(_) => {
                                connection.abort();
                                context
                                    .state
                                    .record_listener_status(HostListenerStatus::Error, Some("connection task stop timed out".to_owned()));
                            }
                        }
                        context
                            .state
                            .record_listener_status(HostListenerStatus::Stopped, None);
                        break 'run;
                    }
                }
                maybe = control_rx.recv() => {
                    match maybe {
                        Some(request) => {
                            if let Err(err) = conn_control_tx.send(request).await {
                                err.0.fail("host connection control channel is closed");
                            }
                        }
                        None => {
                            let _ = shutdown.wait_for(|stopped| *stopped).await;
                            break 'run;
                        }
                    }
                }
            }
        }

        append_host_audit(
            &context,
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
                    context
                        .state
                        .record_listener_status(HostListenerStatus::Stopping, None);
                    context
                        .state
                        .record_listener_status(HostListenerStatus::Stopped, None);
                    break 'run;
                }
            }
            maybe = control_rx.recv() => {
                if let Some(request) = maybe {
                    request.fail("host listener is reconnecting");
                } else {
                    break 'run;
                }
            }
        }
    }
}

fn record_connection_result(
    context: &HostContext,
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) {
    match result {
        Ok(Ok(())) => {
            context
                .state
                .record_listener_status(HostListenerStatus::Reconnecting, None);
        }
        Ok(Err(err)) => {
            let reason = err.to_string();
            warn!("host connection failed: {reason}");
            context
                .state
                .record_listener_status(HostListenerStatus::Reconnecting, Some(reason));
        }
        Err(err) => {
            let reason = err.to_string();
            warn!("host connection task failed: {reason}");
            context
                .state
                .record_listener_status(HostListenerStatus::Reconnecting, Some(reason));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> HostConfig {
        let data_home = std::env::temp_dir().join(format!(
            "rcw-host-core-test-{}",
            rcw_common::ids::new_request_id()
        ));
        std::env::set_var("XDG_DATA_HOME", &data_home);
        HostConfig::new()
            .with_server(Some("http://127.0.0.1:9".to_owned()))
            .with_audit_log(Some(data_home.join("audit.jsonl")))
    }

    #[tokio::test]
    async fn managed_listener_controls_are_guarded_and_idempotent() {
        let service = HostService::new(test_config()).unwrap();

        let err = service.close_current_session().await.unwrap_err();
        assert!(err.to_string().contains("host listener is not running"));

        assert!(service.start_listener().await.unwrap().changed);
        assert!(!service.start_listener().await.unwrap().changed);
        assert!(service.stop_listener().await.unwrap().changed);
        assert!(!service.stop_listener().await.unwrap().changed);
    }
}
