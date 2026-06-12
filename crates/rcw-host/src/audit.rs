use rcw_common::audit::{append_jsonl, AuditEvent};
use tracing::warn;

use crate::HostContext;

pub(crate) fn append_host_audit(
    context: &HostContext,
    event: &str,
    request_id: Option<String>,
    session_id: Option<String>,
    command: Option<String>,
    result: Option<&str>,
) {
    let mut audit = AuditEvent::new("host", event);
    audit.machine_id = Some(context.machine_id.clone());
    audit.request_id = request_id;
    audit.session_id = session_id;
    audit.command = command;
    audit.result = result.map(str::to_owned);
    if let Err(err) = append_jsonl(&context.audit_path, &audit) {
        warn!("failed to write host audit log: {err}");
    }
}
