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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub time: String,
    pub side: String,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl AuditEvent {
    pub fn new(side: impl Into<String>, event: impl Into<String>) -> Self {
        Self {
            time: now_rfc3339(),
            side: side.into(),
            event: event.into(),
            machine_id: None,
            host_id: None,
            session_id: None,
            request_id: None,
            command: None,
            audit_label: None,
            result: None,
            duration_ms: None,
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
}
