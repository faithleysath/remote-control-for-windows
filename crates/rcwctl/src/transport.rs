use std::{path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    config,
    protocol::{
        CommandCompletePayload, CommandOutputPayload, CommandRequestPayload, DownloadArgs,
        ErrorPayload, UploadArgs, WireMessage, COMMAND_DOWNLOAD_BEGIN, COMMAND_UPLOAD_BEGIN,
        TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT, TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR,
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

use crate::{controller_config::ControllerConfig, session::SessionStore};

type WsStream = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
type WsSink = futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

#[derive(Debug, Default)]
pub(crate) struct CommandResponse {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
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
    wait: Duration,
}

pub(crate) enum IncomingFrame {
    Text(WireMessage),
    Binary(Vec<u8>),
}

pub(crate) async fn send_command(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    command: &str,
    args: Value,
    wait: Duration,
) -> Result<CommandResponse> {
    send_command_with_terminal(
        config,
        store,
        CommandSend {
            request_id,
            command,
            args,
            terminal_kinds: &[TYPE_COMMAND_COMPLETE],
            wait,
        },
    )
    .await
}

async fn send_command_with_terminal(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    send: CommandSend<'_>,
) -> Result<CommandResponse> {
    let session = store.read_session()?;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: send.command.to_owned(),
        audit_label: config.audit_label.clone(),
        args: send.args,
    };
    let message = WireMessage::new(
        rcw_common::protocol::TYPE_COMMAND_REQUEST,
        Some(send.request_id.to_owned()),
        Some(session.session_id.clone()),
        payload,
    )?;
    let messages =
        send_and_collect(&session.server, message, send.terminal_kinds, send.wait).await?;
    store.touch_session(session)?;
    command_response(messages)
}

pub(crate) async fn send_command_with_upload_file(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    local: &Path,
    args: UploadArgs,
    wait: Duration,
) -> Result<CommandResponse> {
    let session = store.read_session()?;
    let size = args.size;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: COMMAND_UPLOAD_BEGIN.to_owned(),
        audit_label: config.audit_label.clone(),
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
    let mut messages = Vec::new();
    {
        let mut send_file = Box::pin(send_file_binary_chunks(
            &mut sink,
            request_id,
            BinaryKind::UploadChunk,
            local,
            size,
        ));
        loop {
            tokio::select! {
                result = &mut send_file => {
                    result?;
                    break;
                }
                frame = next_message_unbounded(&mut stream) => {
                    let frame = frame?;
                    let terminal = is_terminal_frame(&frame, &[TYPE_UPLOAD_COMPLETE])?;
                    messages.push(frame);
                    if terminal {
                        store.touch_session(session)?;
                        return command_response(messages);
                    }
                }
            }
        }
    }
    messages.extend(collect_until_terminal(&mut stream, &[TYPE_UPLOAD_COMPLETE], wait).await?);
    store.touch_session(session)?;
    command_response(messages)
}

pub(crate) async fn send_command_download_to_file(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    request_id: &str,
    remote: &str,
    mut output: tokio::fs::File,
    wait: Duration,
) -> Result<DownloadStreamResponse> {
    let session = store.read_session()?;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: COMMAND_DOWNLOAD_BEGIN.to_owned(),
        audit_label: config.audit_label.clone(),
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

    let mut hasher = Sha256Accumulator::new();
    let mut bytes_written = 0_u64;
    loop {
        let frame = next_message(&mut stream, wait).await?;
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
                    store.touch_session(session)?;
                    return Ok(DownloadStreamResponse {
                        complete: message.payload_as()?,
                        bytes_written,
                        sha256: hasher.finalize(),
                    });
                }
            }
            IncomingFrame::Binary(bytes) => {
                let frame = BinaryFrame::decode(&bytes)?;
                if frame.request_id != request_id {
                    bail!(
                        "download binary frame request_id mismatch: expected {request_id}, got {}",
                        frame.request_id
                    );
                }
                if frame.kind == BinaryKind::DownloadChunk {
                    output.write_all(&frame.payload).await?;
                    hasher.update(&frame.payload);
                    bytes_written += frame.payload.len() as u64;
                }
            }
        }
    }
}

pub(crate) async fn send_and_collect(
    server: &str,
    message: WireMessage,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<Vec<IncomingFrame>> {
    let (mut sink, mut stream) = connect_control(server).await?;
    send_json(&mut sink, message).await?;
    collect_until_terminal(&mut stream, terminal_kinds, wait).await
}

async fn collect_until_terminal(
    stream: &mut WsStream,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<Vec<IncomingFrame>> {
    let mut messages = Vec::new();
    loop {
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

async fn send_file_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    path: &Path,
    size: u64,
) -> Result<()> {
    let total_sequences = total_sequences_for_len(size)?;
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
        let chunk_len = remaining.min(CHUNK_SIZE as u64) as usize;
        file.read_exact(&mut buffer[..chunk_len])
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
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

async fn connect_control(server: &str) -> Result<(WsSink, WsStream)> {
    let url = config::ws_endpoint_url(server, "/ws/control")?;
    let (ws, _) = connect_async(url)
        .await
        .context("failed to connect to rcw-server control websocket")?;
    Ok(ws.split())
}

async fn send_json(sink: &mut WsSink, message: WireMessage) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(&message)?))
        .await?;
    Ok(())
}

async fn next_message_unbounded(stream: &mut WsStream) -> Result<IncomingFrame> {
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

async fn next_message(stream: &mut WsStream, wait: Duration) -> Result<IncomingFrame> {
    loop {
        let frame = timeout(wait, stream.next())
            .await
            .context("timed out waiting for server response")?
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
                        "stdout" => response.stdout.push_str(&output.data),
                        "stderr" => response.stderr.push_str(&output.data),
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
                    BinaryKind::UploadChunk => {}
                }
            }
        }
    }
    Ok(response)
}

pub(crate) fn last_payload<T>(messages: &[IncomingFrame]) -> Result<T>
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
