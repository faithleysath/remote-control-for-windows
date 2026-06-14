use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Result};
use schemars::JsonSchema;
use serde::Serialize;
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

use crate::cancel::{new_cancel_flag, request_cancel, CancelFlag};

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferKind {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferTaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TransferTaskResult {
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) local_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remote_path: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TransferTaskStatusResult {
    pub(crate) task_id: String,
    pub(crate) status: TransferTaskStatus,
    pub(crate) kind: TransferKind,
    pub(crate) request_id: String,
    pub(crate) started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<TransferTaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

pub(crate) type TransferTasks = Arc<std::sync::Mutex<HashMap<String, TransferTaskRecord>>>;

#[derive(Debug)]
pub(crate) struct TransferTaskRecord {
    snapshot: TransferTaskStatusResult,
    handle: Option<JoinHandle<()>>,
    cleanup_path: Option<PathBuf>,
    local_cancel: CancelFlag,
    remote_started: bool,
}

pub(crate) fn new_transfer_tasks() -> TransferTasks {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

pub(crate) fn insert_transfer_task(
    transfers: &TransferTasks,
    snapshot: TransferTaskStatusResult,
) -> Result<()> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .insert(
            snapshot.task_id.clone(),
            TransferTaskRecord {
                snapshot,
                handle: None,
                cleanup_path: None,
                local_cancel: new_cancel_flag(),
                remote_started: false,
            },
        );
    Ok(())
}

pub(crate) fn set_transfer_cleanup_path(
    transfers: &TransferTasks,
    task_id: &str,
    path: PathBuf,
) -> Result<()> {
    let mut transfers = transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?;
    let record = transfers
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))?;
    record.cleanup_path = Some(path);
    Ok(())
}

pub(crate) fn set_transfer_handle(
    transfers: &TransferTasks,
    task_id: &str,
    handle: JoinHandle<()>,
) -> Result<()> {
    let mut transfers = transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?;
    let record = transfers
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))?;
    record.handle = Some(handle);
    Ok(())
}

pub(crate) fn transfer_cancel_flag(transfers: &TransferTasks, task_id: &str) -> Result<CancelFlag> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .get(task_id)
        .map(|record| Arc::clone(&record.local_cancel))
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))
}

pub(crate) fn mark_transfer_remote_started(transfers: &TransferTasks, task_id: &str) -> Result<()> {
    let mut transfers = transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?;
    let record = transfers
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))?;
    record.remote_started = true;
    Ok(())
}

pub(crate) fn transfer_remote_started(transfers: &TransferTasks, task_id: &str) -> Result<bool> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .get(task_id)
        .map(|record| record.remote_started)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))
}

pub(crate) fn transfer_task_snapshot(
    transfers: &TransferTasks,
    task_id: &str,
) -> Result<TransferTaskStatusResult> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .get(task_id)
        .map(|record| record.snapshot.clone())
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))
}

pub(crate) async fn wait_for_transfer_task(
    transfers: &TransferTasks,
    task_id: &str,
    rx: oneshot::Receiver<TransferTaskStatusResult>,
    wait_ms: u64,
) -> Result<TransferTaskStatusResult> {
    if wait_ms == 0 {
        return transfer_task_snapshot(transfers, task_id);
    }
    match timeout(Duration::from_millis(wait_ms), rx).await {
        Ok(Ok(snapshot)) => fail_if_transfer_failed(snapshot),
        Ok(Err(_)) => fail_if_transfer_failed(transfer_task_snapshot(transfers, task_id)?),
        Err(_) => transfer_task_snapshot(transfers, task_id),
    }
}

pub(crate) fn cancel_transfer_task(
    transfers: &TransferTasks,
    task_id: &str,
) -> Result<TransferTaskStatusResult> {
    let mut transfers = transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?;
    let record = transfers
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))?;
    if record.snapshot.status != TransferTaskStatus::Running {
        return Ok(record.snapshot.clone());
    }
    request_cancel(&record.local_cancel);
    if let Some(handle) = record.handle.take() {
        handle.abort();
    }
    record.snapshot.status = TransferTaskStatus::Cancelled;
    record.snapshot.finished_at = Some(rcw_common::audit::now_rfc3339());
    record.snapshot.error = Some("transfer cancelled".to_owned());
    if let Some(path) = &record.cleanup_path {
        let _ = std::fs::remove_file(path);
    }
    Ok(record.snapshot.clone())
}

fn fail_if_transfer_failed(snapshot: TransferTaskStatusResult) -> Result<TransferTaskStatusResult> {
    if snapshot.status == TransferTaskStatus::Failed {
        bail!(
            "{}",
            snapshot
                .error
                .clone()
                .unwrap_or_else(|| "transfer failed".to_owned())
        );
    }
    Ok(snapshot)
}

pub(crate) fn finish_transfer_snapshot(
    task_id: String,
    kind: TransferKind,
    request_id: String,
    started_at: String,
    result: Result<TransferTaskResult>,
) -> TransferTaskStatusResult {
    match result {
        Ok(result) => TransferTaskStatusResult {
            task_id,
            status: TransferTaskStatus::Completed,
            kind,
            request_id,
            started_at,
            finished_at: Some(rcw_common::audit::now_rfc3339()),
            result: Some(result),
            error: None,
        },
        Err(error) => {
            let cancelled = is_cancelled_error(&error);
            TransferTaskStatusResult {
                task_id,
                status: if cancelled {
                    TransferTaskStatus::Cancelled
                } else {
                    TransferTaskStatus::Failed
                },
                kind,
                request_id,
                started_at,
                finished_at: Some(rcw_common::audit::now_rfc3339()),
                result: None,
                error: Some(format_error(error)),
            }
        }
    }
}

pub(crate) fn set_transfer_snapshot(transfers: &TransferTasks, snapshot: TransferTaskStatusResult) {
    if let Ok(mut transfers) = transfers.lock() {
        if let Some(record) = transfers.get_mut(&snapshot.task_id) {
            if record.snapshot.status == TransferTaskStatus::Cancelled {
                return;
            }
            record.snapshot = snapshot;
            record.handle = None;
        }
    }
}

fn format_error(error: anyhow::Error) -> String {
    format!("{error:#}")
}

fn is_cancelled_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("operation cancelled")
            || message.contains("request cancelled")
            || message.contains("command cancelled")
            || message.contains("Cancelled:")
    })
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;
    use crate::cancel::is_cancelled;

    fn running_transfer_snapshot(task_id: &str) -> TransferTaskStatusResult {
        TransferTaskStatusResult {
            task_id: task_id.to_owned(),
            status: TransferTaskStatus::Running,
            kind: TransferKind::Upload,
            request_id: "request".to_owned(),
            started_at: "started".to_owned(),
            finished_at: None,
            result: None,
            error: None,
        }
    }

    #[test]
    fn finish_transfer_snapshot_treats_local_cancel_as_cancelled() {
        let snapshot = finish_transfer_snapshot(
            "task".to_owned(),
            TransferKind::Upload,
            "request".to_owned(),
            "started".to_owned(),
            Err(anyhow!("operation cancelled")),
        );

        assert_eq!(snapshot.status, TransferTaskStatus::Cancelled);
        assert!(snapshot.result.is_none());
        assert!(snapshot.error.unwrap().contains("operation cancelled"));
    }

    #[test]
    fn cancel_transfer_task_sets_local_cancel_and_preserves_cancelled_snapshot() {
        let transfers = new_transfer_tasks();
        insert_transfer_task(&transfers, running_transfer_snapshot("task")).unwrap();
        let cancel = transfer_cancel_flag(&transfers, "task").unwrap();

        let cancelled = cancel_transfer_task(&transfers, "task").unwrap();
        assert_eq!(cancelled.status, TransferTaskStatus::Cancelled);
        assert!(is_cancelled(&cancel));

        let failed = finish_transfer_snapshot(
            "task".to_owned(),
            TransferKind::Upload,
            "request".to_owned(),
            "started".to_owned(),
            Err(anyhow!("operation cancelled")),
        );
        set_transfer_snapshot(&transfers, failed);

        let current = transfer_task_snapshot(&transfers, "task").unwrap();
        assert_eq!(current.status, TransferTaskStatus::Cancelled);
        assert_eq!(current.error.as_deref(), Some("transfer cancelled"));
    }
}
