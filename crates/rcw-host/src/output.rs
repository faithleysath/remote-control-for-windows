use std::path::Path;

use anyhow::Result;
use futures_util::SinkExt;
use rcw_common::{
    protocol::{
        CommandCompletePayload, CommandOutputPayload, ErrorCode, ErrorPayload, WireMessage,
        TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT, TYPE_ERROR,
    },
    transfer::{chunk_binary, BinaryKind, FileBinaryFrameReader},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub(crate) type WsSink =
    futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

pub(crate) async fn send_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    bytes: &[u8],
) -> Result<()> {
    for frame in chunk_binary(request_id, kind, bytes)? {
        sink.send(Message::Binary(frame)).await?;
    }
    Ok(())
}

pub(crate) async fn send_file_binary_chunks(
    sink: &mut WsSink,
    request_id: &str,
    kind: BinaryKind,
    path: &Path,
    size: u64,
) -> Result<String> {
    let mut reader = FileBinaryFrameReader::new(path, size, request_id, kind)?;
    while let Some(frame) = reader.next_frame()? {
        sink.send(Message::Binary(frame)).await?;
    }
    Ok(reader.finalize_sha256())
}

pub(crate) async fn send_output(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    stream: &str,
    data: &str,
) -> Result<()> {
    send_json(
        sink,
        WireMessage::new(
            TYPE_COMMAND_OUTPUT,
            Some(request_id.to_owned()),
            session_id,
            CommandOutputPayload {
                stream: stream.to_owned(),
                data: data.to_owned(),
            },
        )?,
    )
    .await
}

pub(crate) async fn send_complete(
    sink: &mut WsSink,
    request_id: &str,
    session_id: Option<String>,
    payload: CommandCompletePayload,
) -> Result<()> {
    send_complete_kind(sink, TYPE_COMMAND_COMPLETE, request_id, session_id, payload).await
}

pub(crate) async fn send_complete_kind(
    sink: &mut WsSink,
    kind: &str,
    request_id: &str,
    session_id: Option<String>,
    payload: CommandCompletePayload,
) -> Result<()> {
    send_json(
        sink,
        WireMessage::new(kind, Some(request_id.to_owned()), session_id, payload)?,
    )
    .await
}

pub(crate) async fn send_error(
    sink: &mut WsSink,
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) -> Result<()> {
    send_json(
        sink,
        WireMessage::new(
            TYPE_ERROR,
            request_id,
            session_id,
            ErrorPayload {
                code,
                message: message.to_owned(),
            },
        )?,
    )
    .await
}

pub(crate) async fn send_json(sink: &mut WsSink, message: WireMessage) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(&message)?))
        .await?;
    Ok(())
}
