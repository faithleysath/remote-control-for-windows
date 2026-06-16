fn main() {
    const COMMANDS: &[&str] = &[
        "host_snapshot",
        "host_settings",
        "host_save_settings",
        "host_start_listener",
        "host_stop_listener",
        "host_restart_listener",
        "host_copy_connection_info",
        "host_close_current_session",
        "host_cancel_exec_task",
        "host_cancel_transfer_task",
        "host_close_tunnel",
        "host_reveal_audit_location",
    ];

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to build rcw-host-gui Tauri context");
}
