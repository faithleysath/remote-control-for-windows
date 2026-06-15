use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use rcw_common::{
    protocol::{
        CommandCompletePayload, CommandRequestPayload, ErrorCode, UploadArgs, WireMessage,
        COMMAND_UPLOAD_BEGIN, TYPE_UPLOAD_COMPLETE,
    },
    transfer::{
        commit_temp_output_file, create_temp_output_file, temp_output_path,
        total_sequences_for_len, BinaryFrame, BinaryKind, Sha256Accumulator,
    },
};
use tracing::warn;

use crate::{
    audit::{append_host_audit_record, upload_audit_details, CommandAuditDetails, HostAuditRecord},
    output::{send_complete_kind, send_error, WsSink},
    HostContext, HostTaskStatus, HostTransferDirection,
};

pub(crate) const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub(crate) const UPLOAD_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) struct UploadState {
    pub(crate) session_id: Option<String>,
    audit_label: Option<String>,
    args: UploadArgs,
    file: Option<File>,
    temp_path: PathBuf,
    next_sequence: u32,
    total_sequences: u32,
    bytes_written: u64,
    hasher: Sha256Accumulator,
    started_at: String,
    started: Instant,
    last_activity: Instant,
}

impl Drop for UploadState {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.temp_path);
    }
}

pub(crate) fn begin_upload(
    context: &HostContext,
    uploads: &mut HashMap<String, UploadState>,
    message: WireMessage,
    payload: CommandRequestPayload,
) -> Result<()> {
    let request_id = message
        .request_id
        .clone()
        .ok_or_else(|| anyhow!("upload command missing request_id"))?;
    let args: UploadArgs = serde_json::from_value(payload.args)?;
    if args.remote_path.trim().is_empty() {
        return Err(anyhow!("empty upload path"));
    }
    let total_sequences = total_sequences_for_len(args.size)?;
    let remote_path = PathBuf::from(&args.remote_path);
    let temp_path = temp_output_path(&remote_path, &request_id);
    let file = create_temp_output_file(&remote_path, &temp_path, args.overwrite)?;
    let started_at = rcw_common::audit::now_rfc3339();
    let started = Instant::now();
    println!(
        "[{}] upload waiting for chunks request={}",
        started_at, request_id
    );
    let details = upload_audit_details(&args);
    append_upload_audit(
        context,
        UploadAudit {
            event: "command.started",
            request_id: &request_id,
            session_id: message.session_id.clone(),
            command: Some(payload.command.clone()),
            audit_label: payload.audit_label.clone(),
            details: &details,
            result: "started",
            started_at: Some(started_at.clone()),
            finished_at: None,
            duration_ms: None,
            bytes: None,
            error_code: None,
            error_message: None,
        },
    );
    context.state.record_transfer_started(
        request_id.clone(),
        message.session_id.clone(),
        HostTransferDirection::Upload,
        Some(args.size),
    );
    uploads.insert(
        request_id,
        UploadState {
            session_id: message.session_id,
            audit_label: payload.audit_label,
            args,
            file: Some(file),
            temp_path,
            next_sequence: 0,
            total_sequences,
            bytes_written: 0,
            hasher: Sha256Accumulator::new(),
            started_at,
            started,
            last_activity: Instant::now(),
        },
    );
    Ok(())
}

pub(crate) async fn handle_binary_frame(
    context: &HostContext,
    sink: &mut WsSink,
    uploads: &mut HashMap<String, UploadState>,
    bytes: Vec<u8>,
) -> Result<()> {
    let frame = BinaryFrame::decode(&bytes)?;
    if frame.kind != BinaryKind::UploadChunk {
        send_error(
            sink,
            Some(frame.request_id),
            None,
            ErrorCode::UnsupportedCommand,
            "host only accepts upload binary chunks from controller",
        )
        .await?;
        return Ok(());
    }

    let request_id = frame.request_id.clone();
    if !uploads.contains_key(&request_id) {
        send_error(
            sink,
            Some(request_id),
            None,
            ErrorCode::SessionExpired,
            "upload chunk has no active upload request",
        )
        .await?;
        return Ok(());
    }

    let action = {
        let state = uploads
            .get_mut(&request_id)
            .ok_or_else(|| anyhow!("upload state disappeared"))?;
        if frame.total_sequences != state.total_sequences {
            Some((
                state.session_id.clone(),
                state.audit_label.clone(),
                state.started_at.clone(),
                state.started.elapsed().as_millis() as u64,
                ErrorCode::InternalError,
                format!(
                    "upload chunk total sequence mismatch: expected {}, got {}",
                    state.total_sequences, frame.total_sequences
                ),
            ))
        } else if frame.sequence != state.next_sequence {
            Some((
                state.session_id.clone(),
                state.audit_label.clone(),
                state.started_at.clone(),
                state.started.elapsed().as_millis() as u64,
                ErrorCode::InternalError,
                format!(
                    "upload chunk sequence mismatch: expected {}, got {}",
                    state.next_sequence, frame.sequence
                ),
            ))
        } else {
            let file = state
                .file
                .as_mut()
                .ok_or_else(|| anyhow!("upload file handle is closed"))?;
            file.write_all(&frame.payload)?;
            state.hasher.update(&frame.payload);
            state.bytes_written += frame.payload.len() as u64;
            state.next_sequence += 1;
            state.last_activity = Instant::now();
            None
        }
    };

    if let Some((session_id, audit_label, started_at, duration_ms, code, message)) = action {
        uploads.remove(&request_id);
        context.state.record_transfer_completed(
            request_id.clone(),
            session_id.clone(),
            HostTaskStatus::Failed,
            None,
            "failed".to_owned(),
        );
        let mut details =
            CommandAuditDetails::new(crate::audit::category_for_command(COMMAND_UPLOAD_BEGIN));
        details.args_summary = Some("upload chunk rejected".to_owned());
        append_upload_audit(
            context,
            UploadAudit {
                event: "command.complete",
                request_id: &request_id,
                session_id: session_id.clone(),
                command: Some(COMMAND_UPLOAD_BEGIN.to_owned()),
                audit_label,
                details: &details,
                result: "failed",
                started_at: Some(started_at),
                finished_at: Some(rcw_common::audit::now_rfc3339()),
                duration_ms: Some(duration_ms),
                bytes: None,
                error_code: Some(format!("{code:?}")),
                error_message: Some(message.clone()),
            },
        );
        send_error(sink, Some(request_id), session_id, code, &message).await?;
        return Ok(());
    }

    if let Some(state) = uploads.get(&request_id) {
        context.state.record_transfer_progress(
            request_id.clone(),
            state.session_id.clone(),
            state.bytes_written,
        );
    }

    let complete = uploads
        .get(&request_id)
        .map(|state| state.next_sequence == state.total_sequences)
        .unwrap_or(false);
    if complete {
        let state = uploads
            .remove(&request_id)
            .ok_or_else(|| anyhow!("upload disappeared during finalization"))?;
        finalize_upload(context, sink, &request_id, state).await?;
    }
    Ok(())
}

async fn finalize_upload(
    context: &HostContext,
    sink: &mut WsSink,
    request_id: &str,
    mut state: UploadState,
) -> Result<()> {
    if let Some(mut file) = state.file.take() {
        file.flush()?;
    }
    if state.bytes_written != state.args.size {
        let details = upload_audit_details(&state.args);
        context.state.record_transfer_completed(
            request_id.to_owned(),
            state.session_id.clone(),
            HostTaskStatus::Failed,
            Some(state.bytes_written),
            "size_mismatch".to_owned(),
        );
        append_upload_audit(
            context,
            UploadAudit {
                event: "command.complete",
                request_id,
                session_id: state.session_id.clone(),
                command: Some(COMMAND_UPLOAD_BEGIN.to_owned()),
                audit_label: state.audit_label.clone(),
                details: &details,
                result: "failed",
                started_at: Some(state.started_at.clone()),
                finished_at: Some(rcw_common::audit::now_rfc3339()),
                duration_ms: Some(state.started.elapsed().as_millis() as u64),
                bytes: Some(state.bytes_written),
                error_code: Some(format!("{:?}", ErrorCode::ChecksumMismatch)),
                error_message: Some("upload size mismatch".to_owned()),
            },
        );
        send_error(
            sink,
            Some(request_id.to_owned()),
            state.session_id.clone(),
            ErrorCode::ChecksumMismatch,
            "upload size mismatch",
        )
        .await?;
        return Err(anyhow!("upload size mismatch"));
    }
    let actual = std::mem::replace(&mut state.hasher, Sha256Accumulator::new()).finalize();
    if actual != state.args.sha256 {
        let details = upload_audit_details(&state.args);
        context.state.record_transfer_completed(
            request_id.to_owned(),
            state.session_id.clone(),
            HostTaskStatus::Failed,
            Some(state.bytes_written),
            "checksum_mismatch".to_owned(),
        );
        append_upload_audit(
            context,
            UploadAudit {
                event: "command.complete",
                request_id,
                session_id: state.session_id.clone(),
                command: Some(COMMAND_UPLOAD_BEGIN.to_owned()),
                audit_label: state.audit_label.clone(),
                details: &details,
                result: "failed",
                started_at: Some(state.started_at.clone()),
                finished_at: Some(rcw_common::audit::now_rfc3339()),
                duration_ms: Some(state.started.elapsed().as_millis() as u64),
                bytes: Some(state.bytes_written),
                error_code: Some(format!("{:?}", ErrorCode::ChecksumMismatch)),
                error_message: Some(ErrorCode::ChecksumMismatch.message().to_owned()),
            },
        );
        send_error(
            sink,
            Some(request_id.to_owned()),
            state.session_id.clone(),
            ErrorCode::ChecksumMismatch,
            ErrorCode::ChecksumMismatch.message(),
        )
        .await?;
        return Err(anyhow!("upload checksum mismatch"));
    }
    commit_temp_output_file(
        &state.temp_path,
        Path::new(&state.args.remote_path),
        state.args.overwrite,
    )?;
    send_complete_kind(
        sink,
        TYPE_UPLOAD_COMPLETE,
        request_id,
        state.session_id.clone(),
        CommandCompletePayload {
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
            size: Some(state.bytes_written),
            sha256: Some(actual),
            summary: Some(format!("wrote {}", state.args.remote_path)),
        },
    )
    .await?;
    context.state.record_transfer_completed(
        request_id.to_owned(),
        state.session_id.clone(),
        HostTaskStatus::Completed,
        Some(state.bytes_written),
        "ok".to_owned(),
    );
    let details = upload_audit_details(&state.args);
    append_upload_audit(
        context,
        UploadAudit {
            event: "command.complete",
            request_id,
            session_id: state.session_id.clone(),
            command: Some(COMMAND_UPLOAD_BEGIN.to_owned()),
            audit_label: state.audit_label.clone(),
            details: &details,
            result: "ok",
            started_at: Some(state.started_at.clone()),
            finished_at: Some(rcw_common::audit::now_rfc3339()),
            duration_ms: Some(state.started.elapsed().as_millis() as u64),
            bytes: Some(state.bytes_written),
            error_code: None,
            error_message: None,
        },
    );
    Ok(())
}

struct UploadAudit<'a> {
    event: &'a str,
    request_id: &'a str,
    session_id: Option<String>,
    command: Option<String>,
    audit_label: Option<String>,
    details: &'a CommandAuditDetails,
    result: &'a str,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration_ms: Option<u64>,
    bytes: Option<u64>,
    error_code: Option<String>,
    error_message: Option<String>,
}

fn append_upload_audit(context: &HostContext, audit: UploadAudit<'_>) {
    let mut record = HostAuditRecord::new(audit.details.category, audit.event);
    record.request_id = Some(audit.request_id.to_owned());
    record.session_id = audit.session_id;
    record.task_id = Some(audit.request_id.to_owned());
    record.command = audit.command;
    record.command_kind = Some(COMMAND_UPLOAD_BEGIN.to_owned());
    record.audit_label = audit.audit_label;
    record.result = Some(audit.result.to_owned());
    record.started_at = audit.started_at;
    record.finished_at = audit.finished_at;
    record.duration_ms = audit.duration_ms;
    record.error_code = audit.error_code;
    record.error_message = audit.error_message;
    record.args_summary = audit.details.args_summary.clone();
    record.path_summary = audit.details.path_summary.clone();
    record.bytes = audit.bytes.or(audit.details.bytes);
    record.size = audit.details.size;
    record.sha256 = audit.details.sha256.clone();
    append_host_audit_record(context, record);
}

pub(crate) fn remove_uploads_for_session(
    uploads: &mut HashMap<String, UploadState>,
    session_id: &str,
) {
    let before = uploads.len();
    uploads.retain(|_, state| state.session_id.as_deref() != Some(session_id));
    let removed = before.saturating_sub(uploads.len());
    if removed > 0 {
        warn!(session_id = %session_id, removed, "removed pending uploads for closed session");
    }
}

pub(crate) fn prune_idle_uploads(uploads: &mut HashMap<String, UploadState>) {
    let now = Instant::now();
    uploads.retain(|request_id, state| {
        let keep = now.duration_since(state.last_activity) <= UPLOAD_IDLE_TIMEOUT;
        if !keep {
            warn!(request_id = %request_id, "removed idle pending upload");
        }
        keep
    });
}
