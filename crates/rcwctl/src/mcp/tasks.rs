use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Result};
use tokio::{sync::oneshot, time::timeout};

use super::types::{
    TransferKind, TransferTaskResult, TransferTaskStatus, TransferTaskStatusResult,
};

pub(super) type TransferTasks = Arc<std::sync::Mutex<HashMap<String, TransferTaskStatusResult>>>;

pub(super) fn insert_transfer_task(
    transfers: &TransferTasks,
    snapshot: TransferTaskStatusResult,
) -> Result<()> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .insert(snapshot.task_id.clone(), snapshot);
    Ok(())
}

pub(super) fn transfer_task_snapshot(
    transfers: &TransferTasks,
    task_id: &str,
) -> Result<TransferTaskStatusResult> {
    transfers
        .lock()
        .map_err(|_| anyhow!("transfer task lock poisoned"))?
        .get(task_id)
        .cloned()
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
        Err(error) => TransferTaskStatusResult {
            task_id,
            status: TransferTaskStatus::Failed,
            kind,
            request_id,
            started_at,
            finished_at: Some(rcw_common::audit::now_rfc3339()),
            result: None,
            error: Some(format_error(error)),
        },
    }
}

pub(super) fn set_transfer_snapshot(transfers: &TransferTasks, snapshot: TransferTaskStatusResult) {
    if let Ok(mut transfers) = transfers.lock() {
        transfers.insert(snapshot.task_id.clone(), snapshot);
    }
}

fn format_error(error: anyhow::Error) -> String {
    format!("{error:#}")
}
