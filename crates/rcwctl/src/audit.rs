use anyhow::Result;
use rcw_common::audit::{append_jsonl, AuditEvent};

use crate::{controller_config::ControllerConfig, session::project_dirs};

fn audit_path() -> Result<std::path::PathBuf> {
    Ok(project_dirs()?.data_dir().join("audit.jsonl"))
}

pub(crate) fn append_controller_audit(
    config: &ControllerConfig,
    request_id: &str,
    command: &str,
    result: &str,
    duration_ms: u64,
    summary: Option<String>,
) {
    let mut event = AuditEvent::new("controller", "command.invoked");
    event.request_id = Some(request_id.to_owned());
    event.command = Some(command.to_owned());
    event.audit_label = config.audit_label.clone();
    event.result = Some(result.to_owned());
    event.duration_ms = Some(duration_ms);
    event.summary = summary;
    if let Ok(path) = audit_path() {
        let _ = append_jsonl(path, &event);
    }
}
