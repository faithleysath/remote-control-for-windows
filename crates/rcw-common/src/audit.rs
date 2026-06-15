use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::RcwResult;

static AUDIT_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditCategory {
    Host,
    Session,
    Exec,
    Transfer,
    Tunnel,
    Input,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub time: String,
    pub side: String,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<AuditCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl AuditEvent {
    pub fn new(side: impl Into<String>, event: impl Into<String>) -> Self {
        Self {
            time: now_rfc3339(),
            side: side.into(),
            event: event.into(),
            category: None,
            machine_id: None,
            host_id: None,
            session_id: None,
            request_id: None,
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
}

pub fn append_jsonl(path: impl AsRef<Path>, event: &AuditEvent) -> RcwResult<()> {
    let path = path.as_ref();
    let _guard = AUDIT_WRITE_LOCK
        .lock()
        .map_err(|_| crate::RcwError::Other("audit write lock poisoned".to_owned()))?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}

pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

#[cfg(test)]
mod tests {
    use std::{fs, thread};

    use super::*;

    #[test]
    fn append_jsonl_preserves_lines_under_concurrent_writes() {
        let path = std::env::temp_dir().join(format!(
            "rcw-audit-concurrent-{}.jsonl",
            crate::ids::new_request_id()
        ));

        let mut handles = Vec::new();
        for worker in 0..8 {
            let path = path.clone();
            handles.push(thread::spawn(move || {
                for index in 0..50 {
                    let mut event = AuditEvent::new("test", "audit.concurrent");
                    event.request_id = Some(format!("{worker}-{index}"));
                    event.summary = Some("x".repeat(4096));
                    append_jsonl(&path, &event).unwrap();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let contents = fs::read_to_string(&path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 400);
        for line in lines {
            serde_json::from_str::<AuditEvent>(line).unwrap();
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn audit_event_serializes_structured_optional_fields() {
        let mut event = AuditEvent::new("host", "command.complete");
        event.category = Some(AuditCategory::Exec);
        event.request_id = Some("req".to_owned());
        event.task_id = Some("task".to_owned());
        event.command_kind = Some("exec".to_owned());
        event.args_summary = Some("program=pwsh argv_count=2".to_owned());
        event.error_code = Some("cancelled".to_owned());

        let encoded = serde_json::to_value(&event).unwrap();

        assert_eq!(encoded["category"], "exec");
        assert_eq!(encoded["task_id"], "task");
        assert_eq!(encoded["command_kind"], "exec");
        assert_eq!(encoded["args_summary"], "program=pwsh argv_count=2");
        assert_eq!(encoded["error_code"], "cancelled");
    }
}
