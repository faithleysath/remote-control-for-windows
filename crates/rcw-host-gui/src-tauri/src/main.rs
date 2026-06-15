#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rcw_common::config;
use rcw_host_core::{
    HostConfig, HostConnectionInfo, HostEvent, HostEventKind, HostListenerStatus, HostService,
    HostSnapshot,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{broadcast, Mutex};

const DEFAULT_GUI_SERVER_URL: &str = "ws://127.0.0.1:7800";
const SETTINGS_FILE_NAME: &str = "host-gui.json";

type CommandResult<T> = std::result::Result<T, String>;

struct GuiState {
    service: Arc<Mutex<HostService>>,
    settings: Mutex<GuiSettingsState>,
}

#[derive(Debug, Clone)]
struct GuiSettingsState {
    path: PathBuf,
    settings: GuiSettings,
    restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct GuiSettings {
    server_url: String,
    totp_period_seconds: u64,
    audit_log_path: String,
    auto_listen: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct HostSettingsInput {
    server_url: String,
    totp_period_seconds: u64,
    audit_log_path: String,
    auto_listen: bool,
}

#[derive(Debug, Clone, Serialize)]
struct HostSettingsView {
    server_url: String,
    totp_period_seconds: u64,
    audit_log_path: String,
    auto_listen: bool,
    config_path: PathBuf,
    restart_required: bool,
}

#[derive(Debug, Clone, Serialize)]
struct HostActionOutcome {
    changed: bool,
    message: String,
    snapshot: HostSnapshot,
}

#[derive(Debug, Clone, Serialize)]
struct HostCopyOutcome {
    copied: bool,
    error: Option<String>,
    info: HostConnectionInfoView,
}

#[derive(Debug, Clone, Serialize)]
struct HostConnectionInfoView {
    server_url: String,
    machine_id: String,
    host_id: String,
    totp: String,
    totp_period_seconds: u64,
    clipboard_text: String,
}

#[tauri::command]
async fn host_snapshot(state: State<'_, GuiState>) -> CommandResult<HostSnapshot> {
    Ok(state.service.lock().await.snapshot())
}

#[tauri::command]
async fn host_settings(state: State<'_, GuiState>) -> CommandResult<HostSettingsView> {
    let settings = state.settings.lock().await;
    Ok(settings.view())
}

#[tauri::command]
async fn host_save_settings(
    app: AppHandle,
    input: HostSettingsInput,
    state: State<'_, GuiState>,
) -> CommandResult<HostSettingsView> {
    let settings = GuiSettings::from_input(input)?;
    let (path, previous_settings) = {
        let stored = state.settings.lock().await;
        (stored.path.clone(), stored.settings.clone())
    };
    write_gui_settings(&path, &settings)?;

    let mut applied = false;
    let mut restart_required = false;
    {
        let mut service = state.service.lock().await;
        let snapshot = service.snapshot();
        if snapshot.listener.status == HostListenerStatus::Stopped {
            service
                .reconfigure_stopped(settings.to_host_config())
                .map_err(to_command_error)?;
            applied = true;
        } else if settings.runtime_differs_from(&snapshot)
            || settings.clears_explicit_audit_path(&previous_settings)
        {
            restart_required = true;
        }
    }

    let view = {
        let mut stored = state.settings.lock().await;
        stored.settings = settings;
        stored.restart_required = restart_required;
        stored.view()
    };

    if applied {
        forward_host_events(app, state.service.clone());
    }
    Ok(view)
}

#[tauri::command]
async fn host_start_listener(
    app: AppHandle,
    state: State<'_, GuiState>,
) -> CommandResult<HostActionOutcome> {
    let settings = state.settings.lock().await.settings.clone();
    let mut applied = false;
    let outcome = {
        let mut service = state.service.lock().await;
        if service.snapshot().listener.status == HostListenerStatus::Stopped {
            service
                .reconfigure_stopped(settings.to_host_config())
                .map_err(to_command_error)?;
            applied = true;
        }
        let outcome = service.start_listener().await.map_err(to_command_error)?;
        HostActionOutcome {
            changed: outcome.changed,
            message: if outcome.changed {
                "Listener started".to_owned()
            } else {
                "Listener was already running".to_owned()
            },
            snapshot: service.snapshot(),
        }
    };

    if applied {
        state.settings.lock().await.restart_required = false;
        forward_host_events(app, state.service.clone());
    }
    Ok(outcome)
}

#[tauri::command]
async fn host_stop_listener(
    app: AppHandle,
    state: State<'_, GuiState>,
) -> CommandResult<HostActionOutcome> {
    let (settings, should_apply_pending_settings) = {
        let settings = state.settings.lock().await;
        (settings.settings.clone(), settings.restart_required)
    };
    let mut applied_pending_settings = false;
    let outcome = {
        let mut service = state.service.lock().await;
        let outcome = service.stop_listener().await.map_err(to_command_error)?;
        if should_apply_pending_settings {
            service
                .reconfigure_stopped(settings.to_host_config())
                .map_err(to_command_error)?;
            applied_pending_settings = true;
        }
        HostActionOutcome {
            changed: outcome.changed,
            message: match (outcome.changed, applied_pending_settings) {
                (true, true) => "Listener stopped and saved settings applied".to_owned(),
                (true, false) => "Listener stopped".to_owned(),
                (false, true) => "Saved settings applied".to_owned(),
                (false, false) => "Listener was already stopped".to_owned(),
            },
            snapshot: service.snapshot(),
        }
    };

    if applied_pending_settings {
        state.settings.lock().await.restart_required = false;
        forward_host_events(app, state.service.clone());
    }
    Ok(outcome)
}

#[tauri::command]
async fn host_restart_listener(
    app: AppHandle,
    state: State<'_, GuiState>,
) -> CommandResult<HostActionOutcome> {
    let settings = state.settings.lock().await.settings.clone();
    let outcome = {
        let mut service = state.service.lock().await;
        let outcome = service
            .restart_with_config(settings.to_host_config())
            .await
            .map_err(to_command_error)?;
        HostActionOutcome {
            changed: outcome.changed,
            message: if outcome.changed {
                "Listener restarted with saved settings".to_owned()
            } else {
                "Listener started with saved settings".to_owned()
            },
            snapshot: service.snapshot(),
        }
    };

    state.settings.lock().await.restart_required = false;
    forward_host_events(app, state.service.clone());
    Ok(outcome)
}

#[tauri::command]
async fn host_close_current_session(
    state: State<'_, GuiState>,
) -> CommandResult<HostActionOutcome> {
    let outcome = {
        let service = state.service.lock().await;
        let outcome = service
            .close_current_session()
            .await
            .map_err(to_command_error)?;
        HostActionOutcome {
            changed: outcome.closed,
            message: match (&outcome.session_id, outcome.closed) {
                (Some(session_id), true) => format!("Session {session_id} close requested"),
                (_, true) => "Current session close requested".to_owned(),
                _ => "No active session to close".to_owned(),
            },
            snapshot: service.snapshot(),
        }
    };
    Ok(outcome)
}

#[tauri::command]
async fn host_copy_connection_info(state: State<'_, GuiState>) -> CommandResult<HostCopyOutcome> {
    let service = state.service.lock().await;
    match service.copy_connection_info() {
        Ok(info) => Ok(HostCopyOutcome {
            copied: true,
            error: None,
            info: HostConnectionInfoView::from_info(info),
        }),
        Err(err) => Ok(HostCopyOutcome {
            copied: false,
            error: Some(err.to_string()),
            info: HostConnectionInfoView::from_info(service.connection_info()),
        }),
    }
}

fn main() {
    tracing_subscriber::fmt().compact().init();

    tauri::Builder::default()
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join(SETTINGS_FILE_NAME);
            let settings = load_gui_settings(&settings_path)?;
            let auto_listen = settings.auto_listen;
            let service = Arc::new(Mutex::new(HostService::new(settings.to_host_config())?));
            forward_host_events(app.handle().clone(), service.clone());
            if auto_listen {
                start_host_listener(service.clone());
            }
            app.manage(GuiState {
                service,
                settings: Mutex::new(GuiSettingsState {
                    path: settings_path,
                    settings,
                    restart_required: false,
                }),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            host_snapshot,
            host_settings,
            host_save_settings,
            host_start_listener,
            host_stop_listener,
            host_restart_listener,
            host_copy_connection_info,
            host_close_current_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running rcw-host-gui");
}

impl GuiSettingsState {
    fn view(&self) -> HostSettingsView {
        HostSettingsView {
            server_url: self.settings.server_url.clone(),
            totp_period_seconds: self.settings.totp_period_seconds,
            audit_log_path: self.settings.audit_log_path.clone(),
            auto_listen: self.settings.auto_listen,
            config_path: self.path.clone(),
            restart_required: self.restart_required,
        }
    }
}

impl GuiSettings {
    fn from_environment() -> Self {
        Self {
            server_url: config::resolve_server_url(None)
                .unwrap_or_else(|_| DEFAULT_GUI_SERVER_URL.to_owned()),
            totp_period_seconds: config::resolve_totp_period_seconds(None)
                .unwrap_or(config::DEFAULT_TOTP_PERIOD_SECONDS),
            audit_log_path: String::new(),
            auto_listen: true,
        }
    }

    fn from_input(input: HostSettingsInput) -> CommandResult<Self> {
        let settings = Self {
            server_url: input.server_url.trim().to_owned(),
            totp_period_seconds: input.totp_period_seconds,
            audit_log_path: input.audit_log_path.trim().to_owned(),
            auto_listen: input.auto_listen,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> CommandResult<()> {
        if self.server_url.is_empty() {
            return Err("server URL is required".to_owned());
        }
        config::ws_endpoint_url(&self.server_url, "/ws/host").map_err(to_command_error)?;
        config::resolve_totp_period_seconds(Some(self.totp_period_seconds))
            .map_err(to_command_error)?;
        Ok(())
    }

    fn to_host_config(&self) -> HostConfig {
        HostConfig::new()
            .with_server(Some(self.server_url.clone()))
            .with_totp_period_seconds(Some(self.totp_period_seconds))
            .with_audit_log(non_empty_path(&self.audit_log_path))
    }

    fn runtime_differs_from(&self, snapshot: &HostSnapshot) -> bool {
        self.server_url != snapshot.server_url
            || self.totp_period_seconds != snapshot.totp.period_seconds
            || non_empty_path(&self.audit_log_path).is_some_and(|path| path != snapshot.audit_path)
    }

    fn clears_explicit_audit_path(&self, previous: &GuiSettings) -> bool {
        non_empty_path(&self.audit_log_path).is_none()
            && non_empty_path(&previous.audit_log_path).is_some()
    }
}

impl HostConnectionInfoView {
    fn from_info(info: HostConnectionInfo) -> Self {
        let clipboard_text = info.clipboard_text();
        Self {
            server_url: info.server_url,
            machine_id: info.machine_id,
            host_id: info.host_id,
            totp: info.totp,
            totp_period_seconds: info.totp_period_seconds,
            clipboard_text,
        }
    }
}

fn load_gui_settings(path: &Path) -> anyhow::Result<GuiSettings> {
    match fs::read(path) {
        Ok(data) => {
            let settings: GuiSettings = serde_json::from_slice(&data)?;
            settings.validate().map_err(anyhow::Error::msg)?;
            Ok(settings)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(GuiSettings::from_environment())
        }
        Err(err) => Err(err.into()),
    }
}

fn write_gui_settings(path: &Path, settings: &GuiSettings) -> CommandResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(to_command_error)?;
    }
    let data = serde_json::to_vec_pretty(settings).map_err(to_command_error)?;
    fs::write(path, data).map_err(to_command_error)
}

fn non_empty_path(value: &str) -> Option<PathBuf> {
    (!value.trim().is_empty()).then(|| PathBuf::from(value.trim()))
}

fn start_host_listener(service: Arc<Mutex<HostService>>) {
    tauri::async_runtime::spawn(async move {
        let service = service.lock().await;
        if let Err(err) = service.start_listener().await {
            tracing::warn!("failed to start host listener: {err}");
        }
    });
}

fn forward_host_events(app: AppHandle, service: Arc<Mutex<HostService>>) {
    tauri::async_runtime::spawn(async move {
        let mut events = service.lock().await.subscribe_events();
        loop {
            match events.recv().await {
                Ok(event) => {
                    if let Err(err) = app.emit("host-event", event) {
                        tracing::warn!("failed to emit host event to frontend: {err}");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let event = HostEvent {
                        time: rcw_common::audit::now_rfc3339(),
                        kind: HostEventKind::ErrorRecorded,
                        request_id: None,
                        session_id: None,
                        command: None,
                        status: Some("lagged".to_owned()),
                        summary: Some(format!("host event stream skipped {skipped} events")),
                    };
                    if let Err(err) = app.emit("host-event", event) {
                        tracing::warn!("failed to emit host event lag warning: {err}");
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn to_command_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_validates_settings_input() {
        let settings = GuiSettings::from_input(HostSettingsInput {
            server_url: " https://example.com/base ".to_owned(),
            totp_period_seconds: 90,
            audit_log_path: " /tmp/host-audit.jsonl ".to_owned(),
            auto_listen: false,
        })
        .unwrap();

        assert_eq!(settings.server_url, "https://example.com/base");
        assert_eq!(settings.totp_period_seconds, 90);
        assert_eq!(settings.audit_log_path, "/tmp/host-audit.jsonl");
        assert!(!settings.auto_listen);
    }

    #[test]
    fn rejects_empty_server_url() {
        let err = GuiSettings::from_input(HostSettingsInput {
            server_url: " ".to_owned(),
            totp_period_seconds: 120,
            audit_log_path: String::new(),
            auto_listen: true,
        })
        .unwrap_err();

        assert_eq!(err, "server URL is required");
    }
}
