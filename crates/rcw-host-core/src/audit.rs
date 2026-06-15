use std::fmt::Write as _;

use rcw_common::{
    audit::{append_jsonl, AuditCategory, AuditEvent},
    protocol::{
        DownloadArgs, ExecArgs, KeyboardKeyArgs, KeyboardTypeArgs, MouseClickArgs, MouseMoveArgs,
        MouseScrollArgs, ScreenshotArgs, TunnelDirection, TunnelOpenPayload, UploadArgs,
        COMMAND_DOWNLOAD_BEGIN, COMMAND_EXEC, COMMAND_KEYBOARD_KEY, COMMAND_KEYBOARD_TYPE,
        COMMAND_MOUSE_CLICK, COMMAND_MOUSE_MOVE, COMMAND_MOUSE_SCROLL, COMMAND_SCREENSHOT,
        COMMAND_UPLOAD_BEGIN, COMMAND_WINDOWS,
    },
};
use serde_json::Value;
use tracing::warn;

use crate::HostContext;

const MAX_AUDIT_TEXT_LEN: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct HostAuditRecord {
    pub(crate) event: String,
    pub(crate) category: AuditCategory,
    pub(crate) request_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) command_kind: Option<String>,
    pub(crate) audit_label: Option<String>,
    pub(crate) controller_label: Option<String>,
    pub(crate) result: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) error_message: Option<String>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) bytes: Option<u64>,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) args_summary: Option<String>,
    pub(crate) path_summary: Option<String>,
    pub(crate) summary: Option<String>,
}

impl HostAuditRecord {
    pub(crate) fn new(category: AuditCategory, event: impl Into<String>) -> Self {
        Self {
            event: event.into(),
            category,
            request_id: None,
            session_id: None,
            task_id: None,
            command: None,
            command_kind: None,
            audit_label: None,
            controller_label: None,
            result: None,
            error_code: None,
            error_message: None,
            duration_ms: None,
            bytes: None,
            size: None,
            sha256: None,
            started_at: None,
            finished_at: None,
            args_summary: None,
            path_summary: None,
            summary: None,
        }
    }

    fn into_audit_event(self, machine_id: String, host_id: String) -> AuditEvent {
        let mut audit = AuditEvent::new("host", self.event);
        audit.category = Some(self.category);
        audit.machine_id = Some(machine_id);
        audit.host_id = Some(host_id);
        audit.request_id = self.request_id;
        audit.session_id = self.session_id;
        audit.task_id = self.task_id;
        audit.command = self.command;
        audit.command_kind = self.command_kind;
        audit.audit_label = self.audit_label.map(|label| sanitize_audit_text(&label));
        audit.controller_label = self
            .controller_label
            .map(|label| sanitize_audit_text(&label));
        audit.result = self.result.map(|result| sanitize_audit_text(&result));
        audit.error_code = self.error_code.map(|code| sanitize_audit_text(&code));
        audit.error_message = self
            .error_message
            .map(|message| sanitize_audit_text(&message));
        audit.duration_ms = self.duration_ms;
        audit.bytes = self.bytes;
        audit.size = self.size;
        audit.sha256 = self.sha256.map(|sha256| sanitize_audit_text(&sha256));
        audit.started_at = self.started_at;
        audit.finished_at = self.finished_at;
        audit.args_summary = self
            .args_summary
            .map(|summary| sanitize_audit_text(&summary));
        audit.path_summary = self
            .path_summary
            .map(|summary| sanitize_audit_text(&summary));
        audit.summary = self.summary.map(|summary| sanitize_audit_text(&summary));
        audit
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandAuditDetails {
    pub(crate) category: AuditCategory,
    pub(crate) args_summary: Option<String>,
    pub(crate) path_summary: Option<String>,
    pub(crate) bytes: Option<u64>,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
}

impl CommandAuditDetails {
    pub(crate) fn new(category: AuditCategory) -> Self {
        Self {
            category,
            args_summary: None,
            path_summary: None,
            bytes: None,
            size: None,
            sha256: None,
        }
    }
}

pub(crate) fn append_host_audit(
    context: &HostContext,
    event: &str,
    request_id: Option<String>,
    session_id: Option<String>,
    command: Option<String>,
    result: Option<&str>,
) {
    let category = command
        .as_deref()
        .map(category_for_command)
        .unwrap_or_else(|| category_for_event(event));
    let mut record = HostAuditRecord::new(category, event);
    record.request_id = request_id;
    record.session_id = session_id;
    record.command_kind = command.clone();
    record.command = command;
    record.result = result.map(str::to_owned);
    append_host_audit_record(context, record);
}

pub(crate) fn append_host_audit_record(context: &HostContext, record: HostAuditRecord) {
    let audit = record.into_audit_event(context.machine_id.clone(), context.host_id.clone());
    if let Err(err) = append_jsonl(&context.audit_path, &audit) {
        warn!("failed to write host audit log: {err}");
    }
}

pub(crate) fn category_for_command(command: &str) -> AuditCategory {
    match command {
        COMMAND_EXEC => AuditCategory::Exec,
        COMMAND_UPLOAD_BEGIN | COMMAND_DOWNLOAD_BEGIN => AuditCategory::Transfer,
        COMMAND_MOUSE_MOVE
        | COMMAND_MOUSE_CLICK
        | COMMAND_MOUSE_SCROLL
        | COMMAND_KEYBOARD_TYPE
        | COMMAND_KEYBOARD_KEY => AuditCategory::Input,
        COMMAND_SCREENSHOT | COMMAND_WINDOWS => AuditCategory::Host,
        _ => AuditCategory::Host,
    }
}

pub(crate) fn command_audit_details(command: &str, args: &Value) -> CommandAuditDetails {
    match command {
        COMMAND_EXEC => summarize_exec_args(args),
        COMMAND_UPLOAD_BEGIN => serde_json::from_value::<UploadArgs>(args.clone())
            .map(|args| upload_audit_details(&args))
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_DOWNLOAD_BEGIN => serde_json::from_value::<DownloadArgs>(args.clone())
            .map(|args| download_audit_details(&args))
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_SCREENSHOT => serde_json::from_value::<ScreenshotArgs>(args.clone())
            .map(|args| {
                let mut details = CommandAuditDetails::new(AuditCategory::Host);
                details.args_summary = Some(match args.display {
                    Some(display) => format!("display={display} format={}", args.format),
                    None => format!("display=primary format={}", args.format),
                });
                details
            })
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_MOUSE_MOVE => serde_json::from_value::<MouseMoveArgs>(args.clone())
            .map(|args| {
                let mut details = CommandAuditDetails::new(AuditCategory::Input);
                details.args_summary = Some(format!("x={} y={}", args.x, args.y));
                details
            })
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_MOUSE_CLICK => serde_json::from_value::<MouseClickArgs>(args.clone())
            .map(|args| {
                let mut details = CommandAuditDetails::new(AuditCategory::Input);
                details.args_summary =
                    Some(format!("x={} y={} button={}", args.x, args.y, args.button));
                details
            })
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_MOUSE_SCROLL => serde_json::from_value::<MouseScrollArgs>(args.clone())
            .map(|args| {
                let mut details = CommandAuditDetails::new(AuditCategory::Input);
                details.args_summary = Some(format!("delta={}", args.delta));
                details
            })
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_KEYBOARD_TYPE => serde_json::from_value::<KeyboardTypeArgs>(args.clone())
            .map(|args| {
                let mut details = CommandAuditDetails::new(AuditCategory::Input);
                details.args_summary = Some(format!(
                    "text_len={} text_bytes={}",
                    args.text.chars().count(),
                    args.text.len()
                ));
                details.bytes = Some(args.text.len() as u64);
                details
            })
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_KEYBOARD_KEY => serde_json::from_value::<KeyboardKeyArgs>(args.clone())
            .map(|args| {
                let mut details = CommandAuditDetails::new(AuditCategory::Input);
                details.args_summary = Some(format!("key={}", sanitize_audit_text(&args.key)));
                details
            })
            .unwrap_or_else(|_| generic_command_details(command, args)),
        COMMAND_WINDOWS => {
            let mut details = CommandAuditDetails::new(AuditCategory::Host);
            details.args_summary = Some("no_args".to_owned());
            details
        }
        _ => generic_command_details(command, args),
    }
}

pub(crate) fn upload_audit_details(args: &UploadArgs) -> CommandAuditDetails {
    let mut details = CommandAuditDetails::new(AuditCategory::Transfer);
    details.args_summary = Some(format!("overwrite={} size={}", args.overwrite, args.size));
    details.path_summary = Some(path_summary(&args.remote_path));
    details.size = Some(args.size);
    details.sha256 = Some(args.sha256.clone());
    details
}

pub(crate) fn download_audit_details(args: &DownloadArgs) -> CommandAuditDetails {
    let mut details = CommandAuditDetails::new(AuditCategory::Transfer);
    details.args_summary = Some("read file".to_owned());
    details.path_summary = Some(path_summary(&args.remote_path));
    details
}

pub(crate) fn tunnel_open_args_summary(payload: &TunnelOpenPayload) -> String {
    let direction = match payload.direction {
        TunnelDirection::Local => "local",
        TunnelDirection::Remote => "remote",
    };
    format!(
        "direction={direction} listen={}:{} target={}:{} idle_timeout_ms={}",
        sanitize_audit_text(&payload.listen_addr),
        payload.listen_port,
        sanitize_audit_text(&payload.target_host),
        payload.target_port,
        payload.idle_timeout_ms
    )
}

pub(crate) fn path_summary(path: &str) -> String {
    format!("basename={}", path_basename(path))
}

pub(crate) fn sanitize_audit_text(value: &str) -> String {
    let compact = compact_text(value);
    if compact.is_empty() {
        return compact;
    }

    let mut redacted = Vec::new();
    let mut redact_next = false;
    for token in compact.split_whitespace() {
        if redact_next {
            redacted.push("[redacted]".to_owned());
            redact_next = false;
            continue;
        }

        let lower = trim_punctuation(token).to_ascii_lowercase();
        if lower == "bearer" {
            redacted.push(token.to_owned());
            redact_next = true;
            continue;
        }

        redacted.push(redact_assignment_token(token));
    }

    truncate_chars(&redacted.join(" "), MAX_AUDIT_TEXT_LEN)
}

fn summarize_exec_args(args: &Value) -> CommandAuditDetails {
    let Ok(args) = serde_json::from_value::<ExecArgs>(args.clone()) else {
        return generic_command_details(COMMAND_EXEC, args);
    };
    let mut details = CommandAuditDetails::new(AuditCategory::Exec);
    let mut summary = format!(
        "program={} argv_count={}",
        path_basename(&args.program),
        args.argv.len()
    );
    if let Some(timeout_ms) = args.timeout_ms {
        let _ = write!(summary, " timeout_ms={timeout_ms}");
    }
    details.args_summary = Some(summary);
    details.path_summary = args
        .cwd
        .as_deref()
        .map(|cwd| format!("cwd_{}", path_summary(cwd)));
    details
}

fn generic_command_details(command: &str, args: &Value) -> CommandAuditDetails {
    let mut details = CommandAuditDetails::new(category_for_command(command));
    details.args_summary = Some(json_shape_summary(args));
    details
}

fn category_for_event(event: &str) -> AuditCategory {
    if event.starts_with("session.") {
        AuditCategory::Session
    } else if event.starts_with("tunnel.") {
        AuditCategory::Tunnel
    } else if event.starts_with("command.") {
        AuditCategory::Exec
    } else if event.contains("error") || event.ends_with(".failed") {
        AuditCategory::Error
    } else {
        AuditCategory::Host
    }
}

fn path_basename(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    let basename = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("<root>");
    sanitize_audit_text(basename)
}

fn json_shape_summary(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(_) => "bool".to_owned(),
        Value::Number(_) => "number".to_owned(),
        Value::String(value) => format!("string_len={}", value.chars().count()),
        Value::Array(items) => format!("array_len={}", items.len()),
        Value::Object(map) => {
            let mut fields = map
                .iter()
                .map(|(key, value)| {
                    if is_sensitive_key(key) {
                        format!("{key}=[redacted]")
                    } else {
                        format!("{key}={}", json_kind(value))
                    }
                })
                .collect::<Vec<_>>();
            fields.sort();
            format!("fields={}", fields.join(","))
        }
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn redact_assignment_token(token: &str) -> String {
    for separator in ['=', ':'] {
        if let Some((key, _value)) = token.split_once(separator) {
            if is_sensitive_key(trim_punctuation(key)) {
                return format!("{key}{separator}[redacted]");
            }
        }
    }
    token.to_owned()
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .replace('-', "_")
        .to_ascii_lowercase();
    normalized == "key"
        || normalized.ends_with("_key")
        || normalized.contains("token")
        || normalized.contains("password")
        || normalized.contains("passwd")
        || normalized.contains("secret")
}

fn compact_text(value: &str) -> String {
    value
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn trim_punctuation(value: &str) -> &str {
    value.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
}

#[cfg(test)]
mod tests {
    use rcw_common::audit::AuditCategory;
    use serde_json::json;

    use super::*;

    #[test]
    fn keyboard_type_audit_records_only_text_length() {
        let details = command_audit_details(
            COMMAND_KEYBOARD_TYPE,
            &json!({"text": "password=super-secret"}),
        );

        assert_eq!(details.category, AuditCategory::Input);
        assert_eq!(
            details.args_summary.as_deref(),
            Some("text_len=21 text_bytes=21")
        );
        assert_eq!(details.bytes, Some(21));
        assert!(!details
            .args_summary
            .as_deref()
            .unwrap()
            .contains("super-secret"));
    }

    #[test]
    fn exec_audit_summarizes_program_without_argv_values() {
        let details = command_audit_details(
            COMMAND_EXEC,
            &json!({
                "program": "C:\\Tools\\pwsh.exe",
                "argv": ["-Command", "Write-Host token=abc"],
                "cwd": "C:\\Users\\Alice\\Documents",
                "timeout_ms": 1000
            }),
        );

        assert_eq!(details.category, AuditCategory::Exec);
        assert_eq!(
            details.args_summary.as_deref(),
            Some("program=pwsh.exe argv_count=2 timeout_ms=1000")
        );
        assert_eq!(
            details.path_summary.as_deref(),
            Some("cwd_basename=Documents")
        );
        assert!(!details.args_summary.unwrap().contains("Write-Host"));
    }

    #[test]
    fn upload_audit_uses_basename_and_structured_size_hash() {
        let args = UploadArgs {
            remote_path: "C:\\Users\\Alice\\Secrets\\report.txt".to_owned(),
            overwrite: false,
            sha256: "abc123".to_owned(),
            size: 42,
        };
        let details = upload_audit_details(&args);

        assert_eq!(details.category, AuditCategory::Transfer);
        assert_eq!(details.path_summary.as_deref(), Some("basename=report.txt"));
        assert_eq!(details.size, Some(42));
        assert_eq!(details.sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn sensitive_assignments_are_redacted() {
        let sanitized =
            sanitize_audit_text("token=abc password:hunter2 api-key=secret Bearer raw-token");

        assert_eq!(
            sanitized,
            "token=[redacted] password:[redacted] api-key=[redacted] Bearer [redacted]"
        );
    }

    #[test]
    fn host_record_maps_to_common_audit_event() {
        let mut record = HostAuditRecord::new(AuditCategory::Exec, "command.complete");
        record.request_id = Some("req".to_owned());
        record.task_id = Some("task".to_owned());
        record.command = Some(COMMAND_EXEC.to_owned());
        record.command_kind = Some(COMMAND_EXEC.to_owned());
        record.args_summary = Some("token=abc".to_owned());

        let event = record.into_audit_event("machine".to_owned(), "host".to_owned());

        assert_eq!(event.category, Some(AuditCategory::Exec));
        assert_eq!(event.machine_id.as_deref(), Some("machine"));
        assert_eq!(event.host_id.as_deref(), Some("host"));
        assert_eq!(event.args_summary.as_deref(), Some("token=[redacted]"));
    }
}
