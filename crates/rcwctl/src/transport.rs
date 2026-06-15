use std::{path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    config,
    protocol::{
        CommandCancelPayload, CommandCancelResultPayload, CommandCompletePayload,
        CommandOutputPayload, CommandRequestPayload, CommandStatusPayload,
        CommandStatusResultPayload, ControlOpenPayload, ControlOpenResultPayload, DownloadArgs,
        ErrorPayload, SessionClosePayload, SessionCloseResultPayload, SessionStatusPayload,
        SessionStatusResultPayload, UploadArgs, WireMessage, COMMAND_DOWNLOAD_BEGIN,
        COMMAND_UPLOAD_BEGIN, MAX_CAPTURED_OUTPUT_BYTES, PROTOCOL_VERSION, TYPE_COMMAND_CANCEL,
        TYPE_COMMAND_CANCEL_RESULT, TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT, TYPE_COMMAND_START,
        TYPE_COMMAND_START_RESULT, TYPE_COMMAND_STATUS, TYPE_COMMAND_STATUS_RESULT,
        TYPE_CONTROL_OPEN, TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR, TYPE_SESSION_CLOSE,
        TYPE_SESSION_CLOSE_RESULT, TYPE_SESSION_STATUS, TYPE_SESSION_STATUS_RESULT,
        TYPE_UPLOAD_COMPLETE,
    },
    transfer::{total_sequences_for_len, BinaryFrame, BinaryKind, Sha256Accumulator, CHUNK_SIZE},
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::{
    cancel::{bail_if_cancelled, CancelFlag},
    commands::RemoteStartHook,
    controller_config::{config_wait_timeout, ControllerConfig},
    session::SessionStore,
};

pub(crate) type WsStream =
    futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
pub(crate) type WsSink =
    futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

#[derive(Debug, Default)]
pub(crate) struct CommandResponse {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) file: Vec<u8>,
    pub(crate) json_stream: String,
    pub(crate) complete: Option<CommandCompletePayload>,
}

pub(crate) struct DownloadStreamResponse {
    pub(crate) complete: CommandCompletePayload,
    pub(crate) bytes_written: u64,
    pub(crate) sha256: String,
}

struct CommandSend<'a> {
    request_id: &'a str,
    command: &'a str,
    args: Value,
    terminal_kinds: &'a [&'a str],
    wait: Option<Duration>,
    cancel: Option<CancelFlag>,
    on_remote_start: Option<RemoteStartHook>,
}

pub(crate) enum IncomingFrame {
    Text(WireMessage),
    Binary(Vec<u8>),
}

struct DownloadReceiver {
    request_id: String,
    hasher: Sha256Accumulator,
    bytes_written: u64,
}

impl DownloadReceiver {
    fn new(request_id: &str) -> Self {
        Self {
            request_id: request_id.to_owned(),
            hasher: Sha256Accumulator::new(),
            bytes_written: 0,
        }
    }

    async fn accept_binary_frame(
        &mut self,
        bytes: &[u8],
        output: &mut tokio::fs::File,
    ) -> Result<()> {
        let frame = BinaryFrame::decode(bytes)?;
        if frame.request_id != self.request_id {
            bail!(
                "download binary frame request_id mismatch: expected {}, got {}",
                self.request_id,
                frame.request_id
            );
        }
        if frame.kind != BinaryKind::DownloadChunk {
            return Ok(());
        }
        output.write_all(&frame.payload).await?;
        self.hasher.update(&frame.payload);
        self.bytes_written += frame.payload.len() as u64;
        Ok(())
    }

    fn finish(self, complete: CommandCompletePayload) -> DownloadStreamResponse {
        DownloadStreamResponse {
            complete,
            bytes_written: self.bytes_written,
            sha256: self.hasher.finalize(),
        }
    }
}

enum UploadSendOutcome {
    Sent(Vec<IncomingFrame>),
    Terminal(Vec<IncomingFrame>),
}

impl UploadSendOutcome {
    fn into_messages(self) -> Vec<IncomingFrame> {
        match self {
            Self::Sent(messages) | Self::Terminal(messages) => messages,
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal(_))
    }
}

pub(crate) struct OpenedSession {
    pub(crate) server: String,
    pub(crate) machine_id: String,
    pub(crate) host_id: String,
    pub(crate) session_id: String,
    pub(crate) session_token: String,
}

#[derive(Clone, Copy)]
pub(crate) struct OpenSessionRequest<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) machine_id: &'a str,
    pub(crate) host_id: Option<&'a str>,
    pub(crate) totp: &'a str,
    pub(crate) explicit_period: Option<u64>,
    pub(crate) force_reconnect: bool,
}

pub(crate) struct ControlClient<'a> {
    config: &'a ControllerConfig,
    store: &'a dyn SessionStore,
}

impl<'a> ControlClient<'a> {
    pub(crate) fn new(config: &'a ControllerConfig, store: &'a dyn SessionStore) -> Self {
        Self { config, store }
    }

    pub(crate) async fn open_session(
        &self,
        request: OpenSessionRequest<'_>,
        wait: Duration,
    ) -> Result<OpenedSession> {
        let server = config::resolve_server_url(self.config.server.as_deref())?;
        let token = config::control_token(self.config.token.as_deref())?;
        let period = config::resolve_totp_period_seconds(request.explicit_period)?;
        let message = WireMessage::new(
            TYPE_CONTROL_OPEN,
            Some(request.request_id.to_owned()),
            None,
            ControlOpenPayload {
                protocol_version: PROTOCOL_VERSION,
                control_token: token,
                machine_id: request.machine_id.to_owned(),
                host_id: request.host_id.map(ToOwned::to_owned),
                totp: request.totp.to_owned(),
                totp_period_seconds: period,
                force_reconnect: request.force_reconnect,
            },
        )?;
        let messages = send_and_collect(
            &server,
            message,
            &[rcw_common::protocol::TYPE_CONTROL_OPEN_RESULT],
            Some(wait),
            None,
            None,
        )
        .await?;
        let result: ControlOpenResultPayload = last_payload(&messages)?;
        Ok(OpenedSession {
            server,
            machine_id: result.machine_id,
            host_id: result.host_id,
            session_id: result.session_id,
            session_token: result.session_token,
        })
    }

    pub(crate) async fn status(
        &self,
        request_id: &str,
        wait: Duration,
    ) -> Result<SessionStatusResultPayload> {
        let session = self.store.read_session()?;
        let message = WireMessage::new(
            TYPE_SESSION_STATUS,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            SessionStatusPayload {
                session_token: session.session_token.clone(),
            },
        )?;
        let messages = send_and_collect(
            &session.server,
            message,
            &[TYPE_SESSION_STATUS_RESULT],
            Some(wait),
            None,
            None,
        )
        .await?;
        let result = last_payload(&messages)?;
        self.store.touch_session(session)?;
        Ok(result)
    }

    pub(crate) async fn close_session(
        &self,
        request_id: &str,
        wait: Duration,
    ) -> Result<SessionCloseResultPayload> {
        let session = self.store.read_session()?;
        let message = WireMessage::new(
            TYPE_SESSION_CLOSE,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            SessionClosePayload {
                session_token: session.session_token.clone(),
            },
        )?;
        let messages = send_and_collect(
            &session.server,
            message,
            &[TYPE_SESSION_CLOSE_RESULT],
            Some(wait),
            None,
            None,
        )
        .await?;
        let result = last_payload(&messages)?;
        self.store.remove_session()?;
        Ok(result)
    }

    pub(crate) async fn command(
        &self,
        request_id: &str,
        command: &str,
        args: Value,
        wait: Duration,
        cancel: Option<CancelFlag>,
        on_remote_start: Option<RemoteStartHook>,
    ) -> Result<CommandResponse> {
        self.command_with_terminal(CommandSend {
            request_id,
            command,
            args,
            terminal_kinds: &[TYPE_COMMAND_COMPLETE],
            wait: Some(wait),
            cancel,
            on_remote_start,
        })
        .await
    }

    pub(crate) async fn cancel_command(&self, request_id: &str) -> Result<()> {
        let session = self.store.read_session()?;
        let message = WireMessage::new(
            TYPE_COMMAND_CANCEL,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            CommandCancelPayload {
                session_token: session.session_token.clone(),
            },
        )?;
        let (mut sink, mut stream) = connect_control(&session.server).await?;
        send_json(&mut sink, message).await?;
        let messages = collect_until_terminal(
            &mut stream,
            &[TYPE_COMMAND_CANCEL_RESULT],
            Some(config_wait_timeout(self.config)?),
            None,
        )
        .await;
        close_control(&mut sink).await;
        let result: CommandCancelResultPayload = last_payload(&messages?)?;
        if !result.ok {
            bail!("command cancel was rejected");
        }
        self.store.touch_session(session)?;
        Ok(())
    }

    pub(crate) async fn start_exec_job(
        &self,
        task_id: &str,
        args: Value,
    ) -> Result<CommandStatusResultPayload> {
        let session = self.store.read_session()?;
        let payload = CommandRequestPayload {
            session_token: session.session_token.clone(),
            command: rcw_common::protocol::COMMAND_EXEC.to_owned(),
            audit_label: self.config.audit_label.clone(),
            args,
        };
        let message = WireMessage::new(
            TYPE_COMMAND_START,
            Some(task_id.to_owned()),
            Some(session.session_id.clone()),
            payload,
        )?;
        let messages = send_and_collect(
            &session.server,
            message,
            &[TYPE_COMMAND_START_RESULT],
            Some(config_wait_timeout(self.config)?),
            None,
            None,
        )
        .await?;
        self.store.touch_session(session)?;
        last_payload(&messages)
    }

    pub(crate) async fn command_status(&self, task_id: &str) -> Result<CommandStatusResultPayload> {
        let session = self.store.read_session()?;
        let message = WireMessage::new(
            TYPE_COMMAND_STATUS,
            Some(task_id.to_owned()),
            Some(session.session_id.clone()),
            CommandStatusPayload {
                session_token: session.session_token.clone(),
                task_id: task_id.to_owned(),
            },
        )?;
        let messages = send_and_collect(
            &session.server,
            message,
            &[TYPE_COMMAND_STATUS_RESULT],
            Some(config_wait_timeout(self.config)?),
            None,
            None,
        )
        .await?;
        self.store.touch_session(session)?;
        last_payload(&messages)
    }

    async fn command_with_terminal(&self, send: CommandSend<'_>) -> Result<CommandResponse> {
        let session = self.store.read_session()?;
        let payload = CommandRequestPayload {
            session_token: session.session_token.clone(),
            command: send.command.to_owned(),
            audit_label: self.config.audit_label.clone(),
            args: send.args,
        };
        let message = WireMessage::new(
            rcw_common::protocol::TYPE_COMMAND_REQUEST,
            Some(send.request_id.to_owned()),
            Some(session.session_id.clone()),
            payload,
        )?;
        let messages = send_and_collect(
            &session.server,
            message,
            send.terminal_kinds,
            send.wait,
            send.cancel.clone(),
            send.on_remote_start,
        )
        .await?;
        self.store.touch_session(session)?;
        command_response(messages)
    }

    pub(crate) async fn upload_file(
        &self,
        request_id: &str,
        local: &Path,
        args: UploadArgs,
        wait: Duration,
        cancel: Option<CancelFlag>,
        on_remote_start: Option<RemoteStartHook>,
    ) -> Result<CommandResponse> {
        let session = self.store.read_session()?;
        let size = args.size;
        let payload = CommandRequestPayload {
            session_token: session.session_token.clone(),
            command: COMMAND_UPLOAD_BEGIN.to_owned(),
            audit_label: self.config.audit_label.clone(),
            args: json!(args),
        };
        let message = WireMessage::new(
            rcw_common::protocol::TYPE_COMMAND_REQUEST,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            payload,
        )?;
        let (mut sink, mut stream) = connect_control(&session.server).await?;
        send_json(&mut sink, message).await?;
        if let Some(on_remote_start) = on_remote_start {
            on_remote_start();
        }
        let outcome = send_upload_chunks_collecting_responses(
            &mut sink,
            &mut stream,
            request_id,
            local,
            size,
            cancel.clone(),
        )
        .await?;
        let terminal = outcome.is_terminal();
        let mut messages = outcome.into_messages();
        if terminal {
            close_control(&mut sink).await;
            self.store.touch_session(session)?;
            return command_response(messages);
        }
        messages.extend(
            collect_until_terminal(&mut stream, &[TYPE_UPLOAD_COMPLETE], Some(wait), cancel)
                .await?,
        );
        close_control(&mut sink).await;
        self.store.touch_session(session)?;
        command_response(messages)
    }

    pub(crate) async fn download_to_file(
        &self,
        request_id: &str,
        remote: &str,
        mut output: tokio::fs::File,
        wait: Duration,
        cancel: Option<CancelFlag>,
        on_remote_start: Option<RemoteStartHook>,
    ) -> Result<DownloadStreamResponse> {
        let session = self.store.read_session()?;
        let payload = CommandRequestPayload {
            session_token: session.session_token.clone(),
            command: COMMAND_DOWNLOAD_BEGIN.to_owned(),
            audit_label: self.config.audit_label.clone(),
            args: json!(DownloadArgs {
                remote_path: remote.to_owned()
            }),
        };
        let message = WireMessage::new(
            rcw_common::protocol::TYPE_COMMAND_REQUEST,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            payload,
        )?;
        let (mut sink, mut stream) = connect_control(&session.server).await?;
        send_json(&mut sink, message).await?;
        if let Some(on_remote_start) = on_remote_start {
            on_remote_start();
        }

        let mut receiver = DownloadReceiver::new(request_id);
        loop {
            bail_if_cancelled(cancel.as_ref())?;
            let frame = next_message(&mut stream, Some(wait)).await?;
            match frame {
                IncomingFrame::Text(message) => {
                    if message.kind == TYPE_ERROR {
                        let error: ErrorPayload = message.payload_as()?;
                        bail!("{:?}: {}", error.code, error.message);
                    }
                    if message.kind == TYPE_DOWNLOAD_COMPLETE {
                        output.flush().await?;
                        output.sync_all().await?;
                        drop(output);
                        close_control(&mut sink).await;
                        self.store.touch_session(session)?;
                        return Ok(receiver.finish(message.payload_as()?));
                    }
                }
                IncomingFrame::Binary(bytes) => {
                    receiver.accept_binary_frame(&bytes, &mut output).await?;
                }
            }
        }
    }
}

async fn send_and_collect(
    server: &str,
    message: WireMessage,
    terminal_kinds: &[&str],
    wait: Option<Duration>,
    cancel: Option<CancelFlag>,
    on_remote_start: Option<RemoteStartHook>,
) -> Result<Vec<IncomingFrame>> {
    bail_if_cancelled(cancel.as_ref())?;
    let (mut sink, mut stream) = connect_control(server).await?;
    send_json(&mut sink, message).await?;
    if let Some(on_remote_start) = on_remote_start {
        on_remote_start();
    }
    let messages = collect_until_terminal(&mut stream, terminal_kinds, wait, cancel).await?;
    close_control(&mut sink).await;
    Ok(messages)
}

async fn collect_until_terminal(
    stream: &mut WsStream,
    terminal_kinds: &[&str],
    wait: Option<Duration>,
    cancel: Option<CancelFlag>,
) -> Result<Vec<IncomingFrame>> {
    let mut messages = Vec::new();
    loop {
        bail_if_cancelled(cancel.as_ref())?;
        let frame = next_message(stream, wait).await?;
        let terminal = is_terminal_frame(&frame, terminal_kinds)?;
        messages.push(frame);
        if terminal {
            return Ok(messages);
        }
    }
}

fn is_terminal_frame(frame: &IncomingFrame, terminal_kinds: &[&str]) -> Result<bool> {
    match frame {
        IncomingFrame::Text(message) => {
            if message.kind == TYPE_ERROR {
                let error: ErrorPayload = message.payload_as()?;
                bail!("{:?}: {}", error.code, error.message);
            }
            Ok(terminal_kinds.iter().any(|kind| *kind == message.kind))
        }
        IncomingFrame::Binary(_) => Ok(false),
    }
}

async fn send_upload_chunks_collecting_responses(
    sink: &mut WsSink,
    stream: &mut WsStream,
    request_id: &str,
    local: &Path,
    size: u64,
    cancel: Option<CancelFlag>,
) -> Result<UploadSendOutcome> {
    let mut messages = Vec::new();
    let mut send_file = Box::pin(send_file_binary_chunks(
        sink,
        request_id,
        BinaryKind::UploadChunk,
        local,
        size,
        cancel.clone(),
    ));
    loop {
        tokio::select! {
            result = &mut send_file => {
                result?;
                return Ok(UploadSendOutcome::Sent(messages));
            }
            frame = next_message_during_upload_send(stream) => {
                let frame = frame?;
                let terminal = is_terminal_frame(&frame, &[TYPE_UPLOAD_COMPLETE])?;
                messages.push(frame);
                if terminal {
                    return Ok(UploadSendOutcome::Terminal(messages));
                }
            }
        }
    }
}

async fn send_file_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    path: &Path,
    size: u64,
    cancel: Option<CancelFlag>,
) -> Result<()> {
    let total_sequences = total_sequences_for_len(size)?;
    bail_if_cancelled(cancel.as_ref())?;
    if size == 0 {
        let frame = BinaryFrame {
            kind,
            request_id: request_id.to_owned(),
            sequence: 0,
            total_sequences,
            payload: Vec::new(),
        }
        .encode()?;
        sink.send(Message::Binary(frame)).await?;
        tokio::task::yield_now().await;
        return Ok(());
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    let mut remaining = size;
    for sequence in 0..total_sequences {
        bail_if_cancelled(cancel.as_ref())?;
        let chunk_len = remaining.min(CHUNK_SIZE as u64) as usize;
        file.read_exact(&mut buffer[..chunk_len])
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        bail_if_cancelled(cancel.as_ref())?;
        let frame = BinaryFrame {
            kind,
            request_id: request_id.to_owned(),
            sequence,
            total_sequences,
            payload: buffer[..chunk_len].to_vec(),
        }
        .encode()?;
        sink.send(Message::Binary(frame)).await?;
        tokio::task::yield_now().await;
        remaining -= chunk_len as u64;
    }
    Ok(())
}

pub(crate) async fn connect_control(server: &str) -> Result<(WsSink, WsStream)> {
    let url = config::ws_endpoint_url(server, "/ws/control")?;
    let (ws, _) = connect_async(url)
        .await
        .context("failed to connect to rcw-server control websocket")?;
    Ok(ws.split())
}

pub(crate) async fn send_json(sink: &mut WsSink, message: WireMessage) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(&message)?))
        .await?;
    Ok(())
}

pub(crate) async fn close_control(sink: &mut WsSink) {
    let _ = sink.send(Message::Close(None)).await;
    let _ = sink.close().await;
}

async fn next_message_during_upload_send(stream: &mut WsStream) -> Result<IncomingFrame> {
    loop {
        let frame = stream
            .next()
            .await
            .ok_or_else(|| anyhow!("server closed control websocket"))??;
        match frame {
            Message::Text(text) => return Ok(IncomingFrame::Text(serde_json::from_str(&text)?)),
            Message::Binary(bytes) => return Ok(IncomingFrame::Binary(bytes)),
            Message::Close(_) => bail!("server closed control websocket"),
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }
}

pub(crate) async fn next_message(
    stream: &mut WsStream,
    wait: Option<Duration>,
) -> Result<IncomingFrame> {
    loop {
        let frame = match wait {
            Some(wait) => timeout(wait, stream.next())
                .await
                .context("timed out waiting for server response")?,
            None => stream.next().await,
        }
        .ok_or_else(|| anyhow!("server closed control websocket"))??;
        match frame {
            Message::Text(text) => return Ok(IncomingFrame::Text(serde_json::from_str(&text)?)),
            Message::Binary(bytes) => return Ok(IncomingFrame::Binary(bytes)),
            Message::Close(_) => bail!("server closed control websocket"),
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }
}

fn command_response(messages: Vec<IncomingFrame>) -> Result<CommandResponse> {
    let mut response = CommandResponse::default();
    for frame in messages {
        match frame {
            IncomingFrame::Text(message) => match message.kind.as_str() {
                TYPE_COMMAND_OUTPUT => {
                    let output: CommandOutputPayload = message.payload_as()?;
                    match output.stream.as_str() {
                        "stdout" => {
                            append_limited_output(
                                &mut response.stdout,
                                &mut response.stdout_truncated,
                                &output.data,
                            );
                        }
                        "stderr" => {
                            append_limited_output(
                                &mut response.stderr,
                                &mut response.stderr_truncated,
                                &output.data,
                            );
                        }
                        "json" => response.json_stream.push_str(&output.data),
                        _ => {}
                    }
                }
                TYPE_COMMAND_COMPLETE | TYPE_UPLOAD_COMPLETE | TYPE_DOWNLOAD_COMPLETE => {
                    response.complete = Some(message.payload_as()?);
                }
                _ => {}
            },
            IncomingFrame::Binary(bytes) => {
                let frame = BinaryFrame::decode(&bytes)?;
                match frame.kind {
                    BinaryKind::DownloadChunk | BinaryKind::ScreenshotChunk => {
                        response.file.extend_from_slice(&frame.payload);
                    }
                    BinaryKind::UploadChunk | BinaryKind::TunnelData => {}
                }
            }
        }
    }
    Ok(response)
}

fn append_limited_output(target: &mut String, truncated: &mut bool, chunk: &str) {
    if *truncated {
        return;
    }
    let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(target.len());
    if remaining == 0 {
        *truncated = true;
        return;
    }
    if chunk.len() <= remaining {
        target.push_str(chunk);
        return;
    }
    let cutoff = chunk
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= remaining)
        .last()
        .unwrap_or(0);
    target.push_str(&chunk[..cutoff]);
    *truncated = true;
}

fn last_payload<T>(messages: &[IncomingFrame]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let message = messages
        .iter()
        .rev()
        .find_map(|frame| match frame {
            IncomingFrame::Text(message) => Some(message),
            IncomingFrame::Binary(_) => None,
        })
        .ok_or_else(|| anyhow!("missing response message"))?;
    Ok(message.payload_as()?)
}

#[cfg(test)]
mod tests {
    use super::{append_limited_output, MAX_CAPTURED_OUTPUT_BYTES};

    #[test]
    fn append_limited_output_truncates_at_capture_limit() {
        let mut output = "a".repeat(MAX_CAPTURED_OUTPUT_BYTES - 2);
        let mut truncated = false;

        append_limited_output(&mut output, &mut truncated, "bcdef");

        assert_eq!(output.len(), MAX_CAPTURED_OUTPUT_BYTES);
        assert!(output.ends_with("bc"));
        assert!(truncated);
    }

    #[test]
    fn append_limited_output_keeps_valid_utf8_when_truncating() {
        let mut output = "a".repeat(MAX_CAPTURED_OUTPUT_BYTES - 1);
        let mut truncated = false;

        append_limited_output(&mut output, &mut truncated, "éx");

        assert_eq!(output.len(), MAX_CAPTURED_OUTPUT_BYTES - 1);
        assert!(truncated);
    }

    #[test]
    fn append_limited_output_ignores_chunks_after_truncation() {
        let mut output = "a".repeat(MAX_CAPTURED_OUTPUT_BYTES);
        let mut truncated = false;

        append_limited_output(&mut output, &mut truncated, "b");
        append_limited_output(&mut output, &mut truncated, "c");

        assert_eq!(output.len(), MAX_CAPTURED_OUTPUT_BYTES);
        assert!(truncated);
    }
}
