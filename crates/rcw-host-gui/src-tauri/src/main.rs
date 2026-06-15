#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use rcw_host_core::{HostConfig, HostEvent, HostEventKind, HostService, HostSnapshot};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::broadcast;

struct GuiState {
    service: Arc<HostService>,
}

#[tauri::command]
fn host_snapshot(state: State<'_, GuiState>) -> HostSnapshot {
    state.service.snapshot()
}

fn main() {
    tracing_subscriber::fmt().compact().init();

    tauri::Builder::default()
        .setup(|app| {
            let service = Arc::new(HostService::new(HostConfig::new())?);
            forward_host_events(app.handle().clone(), service.clone());
            start_host_listener(service.clone());
            app.manage(GuiState { service });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![host_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running rcw-host-gui");
}

fn start_host_listener(service: Arc<HostService>) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) = service.start_listener().await {
            tracing::warn!("failed to start host listener: {err}");
        }
    });
}

fn forward_host_events(app: AppHandle, service: Arc<HostService>) {
    let mut events = service.subscribe_events();
    tauri::async_runtime::spawn(async move {
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
