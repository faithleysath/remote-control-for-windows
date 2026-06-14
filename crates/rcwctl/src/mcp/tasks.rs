use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Result};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

use super::types::{
    ExecTaskStatus, ExecTaskStatusResult, TransferKind, TransferTaskResult, TransferTaskStatus,
    TransferTaskStatusResult,
};
use crate::cancel::{new_cancel_flag, request_cancel, CancelFlag};
use crate::commands::ExecResult;

pub(super) type TransferTasks = Arc<std::sync::Mutex<HashMap<String, TransferTaskRecord>>>;
pub(super) type ExecTasks = Arc<std::sync::Mutex<HashMap<String, ExecTaskRecord>>>;

#[derive(Debug)]
pub(super) struct TransferTaskRecord {
    snapshot: TransferTaskStatusResult,
    handle: Option<JoinHandle<()>>,
    cleanup_path: Option<PathBuf>,
    local_cancel: CancelFlag,
    remote_started: bool,
}

#[derive(Debug)]
pub(super) struct ExecTaskRecord {
    snapshot: ExecTaskStatusResult,
    handle: Option<JoinHandle<()>>,
    local_cancel: CancelFlag,
    remote_started: bool,
}

pub(super) fn new_transfer_tasks() -> TransferTasks {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

pub(super) fn new_exec_tasks() -> ExecTasks {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

pub(super) fn insert_transfer_task(
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

pub(super) fn set_transfer_cleanup_path(
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

pub(super) fn set_transfer_handle(
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

pub(super) fn transfer_cancel_flag(transfers: &TransferTasks, task_id: &str) -> Result<CancelFlag> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .get(task_id)
        .map(|record| Arc::clone(&record.local_cancel))
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))
}

pub(super) fn mark_transfer_remote_started(transfers: &TransferTasks, task_id: &str) -> Result<()> {
    let mut transfers = transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?;
    let record = transfers
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))?;
    record.remote_started = true;
    Ok(())
}

pub(super) fn transfer_remote_started(transfers: &TransferTasks, task_id: &str) -> Result<bool> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .get(task_id)
        .map(|record| record.remote_started)
        .ok_or_else(|| anyhow!("transfer task not found: {task_id}"))
}

pub(super) fn transfer_task_snapshot(
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

pub(super) async fn wait_for_transfer_task(
    transfers: &TransferTasks,
    task_id: &str,
    rx: oneshot::Receiver<TransferTaskStatusResult>,
    wait_timeout_ms: u64,
) -> Result<TransferTaskStatusResult> {
    if wait_timeout_ms == 0 {
        return transfer_task_snapshot(transfers, task_id);
    }
    match timeout(Duration::from_millis(wait_timeout_ms), rx).await {
        Ok(Ok(snapshot)) => fail_if_transfer_failed(snapshot),
        Ok(Err(_)) => fail_if_transfer_failed(transfer_task_snapshot(transfers, task_id)?),
        Err(_) => transfer_task_snapshot(transfers, task_id),
    }
}

pub(super) fn cancel_transfer_task(
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

pub(super) fn finish_transfer_snapshot(
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

pub(super) fn set_transfer_snapshot(transfers: &TransferTasks, snapshot: TransferTaskStatusResult) {
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

pub(super) fn insert_exec_task(
    exec_tasks: &ExecTasks,
    snapshot: ExecTaskStatusResult,
) -> Result<()> {
    exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?
        .insert(
            snapshot.task_id.clone(),
            ExecTaskRecord {
                snapshot,
                handle: None,
                local_cancel: new_cancel_flag(),
                remote_started: false,
            },
        );
    Ok(())
}

pub(super) fn set_exec_handle(
    exec_tasks: &ExecTasks,
    task_id: &str,
    handle: JoinHandle<()>,
) -> Result<()> {
    let mut exec_tasks = exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?;
    let record = exec_tasks
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("exec task not found: {task_id}"))?;
    record.handle = Some(handle);
    Ok(())
}

pub(super) fn exec_cancel_flag(exec_tasks: &ExecTasks, task_id: &str) -> Result<CancelFlag> {
    exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?
        .get(task_id)
        .map(|record| Arc::clone(&record.local_cancel))
        .ok_or_else(|| anyhow!("exec task not found: {task_id}"))
}

pub(super) fn mark_exec_remote_started(exec_tasks: &ExecTasks, task_id: &str) -> Result<()> {
    let mut exec_tasks = exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?;
    let record = exec_tasks
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("exec task not found: {task_id}"))?;
    record.remote_started = true;
    Ok(())
}

pub(super) fn exec_remote_started(exec_tasks: &ExecTasks, task_id: &str) -> Result<bool> {
    exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?
        .get(task_id)
        .map(|record| record.remote_started)
        .ok_or_else(|| anyhow!("exec task not found: {task_id}"))
}

pub(super) fn exec_task_snapshot(
    exec_tasks: &ExecTasks,
    task_id: &str,
) -> Result<ExecTaskStatusResult> {
    exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?
        .get(task_id)
        .map(|record| record.snapshot.clone())
        .ok_or_else(|| anyhow!("exec task not found: {task_id}"))
}

pub(super) async fn wait_for_exec_task(
    exec_tasks: &ExecTasks,
    task_id: &str,
    rx: oneshot::Receiver<ExecTaskStatusResult>,
    wait_timeout_ms: u64,
) -> Result<ExecTaskStatusResult> {
    if wait_timeout_ms == 0 {
        return exec_task_snapshot(exec_tasks, task_id);
    }
    match timeout(Duration::from_millis(wait_timeout_ms), rx).await {
        Ok(Ok(snapshot)) => fail_if_exec_failed(snapshot),
        Ok(Err(_)) => fail_if_exec_failed(exec_task_snapshot(exec_tasks, task_id)?),
        Err(_) => exec_task_snapshot(exec_tasks, task_id),
    }
}

pub(super) fn cancel_exec_task(
    exec_tasks: &ExecTasks,
    task_id: &str,
) -> Result<ExecTaskStatusResult> {
    let mut exec_tasks = exec_tasks
        .lock()
        .map_err(|_| anyhow!("exec task lock poisoned"))?;
    let record = exec_tasks
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("exec task not found: {task_id}"))?;
    if record.snapshot.status != ExecTaskStatus::Running {
        return Ok(record.snapshot.clone());
    }
    request_cancel(&record.local_cancel);
    if let Some(handle) = record.handle.take() {
        handle.abort();
    }
    record.snapshot.status = ExecTaskStatus::Cancelled;
    record.snapshot.finished_at = Some(rcw_common::audit::now_rfc3339());
    record.snapshot.error = Some("exec cancelled".to_owned());
    Ok(record.snapshot.clone())
}

fn fail_if_exec_failed(snapshot: ExecTaskStatusResult) -> Result<ExecTaskStatusResult> {
    if snapshot.status == ExecTaskStatus::Failed {
        bail!(
            "{}",
            snapshot
                .error
                .clone()
                .unwrap_or_else(|| "exec failed".to_owned())
        );
    }
    Ok(snapshot)
}

pub(super) fn finish_exec_snapshot(
    task_id: String,
    request_id: String,
    started_at: String,
    result: Result<ExecResult>,
) -> ExecTaskStatusResult {
    match result {
        Ok(result) => ExecTaskStatusResult {
            task_id,
            status: ExecTaskStatus::Completed,
            request_id,
            started_at,
            finished_at: Some(rcw_common::audit::now_rfc3339()),
            result: Some(result),
            error: None,
        },
        Err(error) => {
            let cancelled = is_cancelled_error(&error);
            ExecTaskStatusResult {
                task_id,
                status: if cancelled {
                    ExecTaskStatus::Cancelled
                } else {
                    ExecTaskStatus::Failed
                },
                request_id,
                started_at,
                finished_at: Some(rcw_common::audit::now_rfc3339()),
                result: None,
                error: Some(format_error(error)),
            }
        }
    }
}

pub(super) fn set_exec_snapshot(exec_tasks: &ExecTasks, snapshot: ExecTaskStatusResult) {
    if let Ok(mut exec_tasks) = exec_tasks.lock() {
        if let Some(record) = exec_tasks.get_mut(&snapshot.task_id) {
            if record.snapshot.status == ExecTaskStatus::Cancelled {
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

    fn running_exec_snapshot(task_id: &str) -> ExecTaskStatusResult {
        ExecTaskStatusResult {
            task_id: task_id.to_owned(),
            status: ExecTaskStatus::Running,
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
    fn finish_exec_snapshot_treats_remote_cancel_as_cancelled() {
        let snapshot = finish_exec_snapshot(
            "task".to_owned(),
            "request".to_owned(),
            "started".to_owned(),
            Err(anyhow!("Cancelled: command killed")),
        );

        assert_eq!(snapshot.status, ExecTaskStatus::Cancelled);
        assert!(snapshot.result.is_none());
        assert!(snapshot.error.unwrap().contains("Cancelled:"));
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

    #[test]
    fn cancel_exec_task_sets_local_cancel_and_preserves_cancelled_snapshot() {
        let exec_tasks = new_exec_tasks();
        insert_exec_task(&exec_tasks, running_exec_snapshot("task")).unwrap();
        let cancel = exec_cancel_flag(&exec_tasks, "task").unwrap();

        let cancelled = cancel_exec_task(&exec_tasks, "task").unwrap();
        assert_eq!(cancelled.status, ExecTaskStatus::Cancelled);
        assert!(is_cancelled(&cancel));

        let failed = finish_exec_snapshot(
            "task".to_owned(),
            "request".to_owned(),
            "started".to_owned(),
            Err(anyhow!("Cancelled: command killed")),
        );
        set_exec_snapshot(&exec_tasks, failed);

        let current = exec_task_snapshot(&exec_tasks, "task").unwrap();
        assert_eq!(current.status, ExecTaskStatus::Cancelled);
        assert_eq!(current.error.as_deref(), Some("exec cancelled"));
    }
}
