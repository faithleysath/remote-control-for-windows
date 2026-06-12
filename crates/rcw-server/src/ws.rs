use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;
use rcw_common::protocol::{ErrorCode, ErrorPayload, WireMessage, TYPE_ERROR};
use tokio::sync::mpsc;
use tracing::{debug, warn};

pub(crate) const HEARTBEAT_INTERVAL_MS: u64 = 15_000;
const OUTBOUND_QUEUE_CAPACITY: usize = 128;

pub(crate) type Tx = mpsc::Sender<Outbound>;

pub(crate) enum Outbound {
    Text(WireMessage),
    Binary(Vec<u8>),
}

pub(crate) fn outbound_channel() -> (Tx, mpsc::Receiver<Outbound>) {
    mpsc::channel(OUTBOUND_QUEUE_CAPACITY)
}

pub(crate) fn spawn_writer(
    mut sender: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Outbound>,
    peer: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    let Some(message) = maybe else {
                        break;
                    };
                    let message = match outbound_to_ws_message(message, peer) {
                        Some(message) => message,
                        None => continue,
                    };
                    if sender.send(message).await.is_err() {
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    if sender.send(Message::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn outbound_to_ws_message(outbound: Outbound, peer: &str) -> Option<Message> {
    match outbound {
        Outbound::Text(message) => match serde_json::to_string(&message) {
            Ok(text) => Some(Message::Text(text)),
            Err(err) => {
                warn!("failed to serialize outbound {peer} message: {err}");
                None
            }
        },
        Outbound::Binary(bytes) => Some(Message::Binary(bytes)),
    }
}

pub(crate) fn send_error(
    tx: &Tx,
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) {
    send_text(tx, make_error(request_id, session_id, code, message));
}

pub(crate) fn make_error(
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) -> WireMessage {
    WireMessage::new(
        TYPE_ERROR,
        request_id,
        session_id,
        ErrorPayload {
            code,
            message: message.to_owned(),
        },
    )
    .expect("error payload serializes")
}

pub(crate) fn send_text(tx: &Tx, message: WireMessage) -> bool {
    send_outbound(tx, Outbound::Text(message))
}

pub(crate) fn send_binary(tx: &Tx, bytes: Vec<u8>) -> bool {
    send_outbound(tx, Outbound::Binary(bytes))
}

fn send_outbound(tx: &Tx, outbound: Outbound) -> bool {
    match tx.try_send(outbound) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!("outbound websocket queue full; dropping message");
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            debug!("outbound websocket closed; dropping message");
            false
        }
    }
}

pub(crate) fn log_websocket_read_error(peer: &str, err: axum::Error) {
    let message = err.to_string();
    if is_unclean_disconnect(&message) {
        debug!("{peer} websocket closed without close handshake: {message}");
    } else {
        warn!("{peer} websocket error: {message}");
    }
}

fn is_unclean_disconnect(message: &str) -> bool {
    message.contains("Connection reset without closing handshake")
        || message.contains("Connection reset by peer")
        || message.contains("Broken pipe")
        || message.contains("connection closed before message completed")
}

#[cfg(test)]
mod tests {
    use super::is_unclean_disconnect;

    #[test]
    fn classifies_common_unclean_disconnects() {
        assert!(is_unclean_disconnect(
            "WebSocket protocol error: Connection reset without closing handshake",
        ));
        assert!(is_unclean_disconnect(
            "IO error: Connection reset by peer (os error 104)",
        ));
        assert!(!is_unclean_disconnect(
            "WebSocket protocol error: invalid frame header"
        ));
    }
}
