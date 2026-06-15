use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use futures_util::SinkExt;
use rcw_common::{
    protocol::{
        ErrorCode, TunnelClosePayload, TunnelCloseResultPayload, TunnelDirection,
        TunnelEndpointSide, TunnelInfo, TunnelOpenPayload, TunnelOpenResultPayload, TunnelStatus,
        TunnelStreamControlPayload, TunnelStreamOpenPayload, TunnelStreamOpenResultPayload,
        WireMessage, DEFAULT_TUNNEL_IDLE_TIMEOUT_MS, TYPE_TUNNEL_CLOSE_RESULT,
        TYPE_TUNNEL_OPEN_RESULT, TYPE_TUNNEL_STREAM_EOF, TYPE_TUNNEL_STREAM_OPEN,
        TYPE_TUNNEL_STREAM_OPEN_RESULT, TYPE_TUNNEL_STREAM_RESET,
    },
    transfer::TunnelDataFrame,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, Mutex},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    audit::append_host_audit,
    output::{send_error, send_json, SharedWsSink},
    HostContext,
};

const TUNNEL_COPY_BUFFER_SIZE: usize = 64 * 1024;
const TUNNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) type HostTunnelTasks = HashMap<String, HostTunnelTask>;

pub(crate) struct HostTunnelTask {
    session_id: Option<String>,
    cancel_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl HostTunnelTask {
    fn abort(self) {
        let _ = self.cancel_tx.send(true);
        self.handle.abort();
    }
}

pub(crate) struct HostTunnelState {
    session_id: Option<String>,
    write_tx: mpsc::Sender<Vec<u8>>,
    handle: JoinHandle<()>,
}

pub(crate) type HostTunnelStreams = Arc<Mutex<HashMap<String, HostTunnelState>>>;

pub(crate) fn new_tunnel_streams() -> HostTunnelStreams {
    Arc::new(Mutex::new(HashMap::new()))
}

struct ReverseListenerConfig {
    tunnel_id: String,
    session_id: Option<String>,
    target_host: String,
    target_port: u16,
    listener: TcpListener,
    streams: HostTunnelStreams,
    cancel_rx: watch::Receiver<bool>,
}

pub(crate) fn prune_finished_tunnel_tasks(tunnels: &mut HostTunnelTasks) {
    tunnels.retain(|_, tunnel| !tunnel.handle.is_finished());
}

pub(crate) fn remove_tunnels_for_session(tunnels: &mut HostTunnelTasks, session_id: &str) {
    let tunnel_ids = tunnels
        .iter()
        .filter(|(_, tunnel)| tunnel.session_id.as_deref() == Some(session_id))
        .map(|(tunnel_id, _)| tunnel_id.clone())
        .collect::<Vec<_>>();
    for tunnel_id in tunnel_ids {
        if let Some(tunnel) = tunnels.remove(&tunnel_id) {
            tunnel.abort();
        }
    }
}

pub(crate) fn abort_tunnel_tasks(tunnels: &mut HostTunnelTasks) {
    for (_, tunnel) in tunnels.drain() {
        tunnel.abort();
    }
}

pub(crate) async fn abort_tunnel_streams(streams: &HostTunnelStreams) {
    let mut streams = streams.lock().await;
    for (_, stream) in streams.drain() {
        stream.handle.abort();
    }
}

pub(crate) async fn abort_tunnel_streams_for_session(
    streams: &HostTunnelStreams,
    session_id: &str,
) {
    let mut streams = streams.lock().await;
    let keys = streams
        .iter()
        .filter(|(_, stream)| stream.session_id.as_deref() == Some(session_id))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in keys {
        if let Some(stream) = streams.remove(&key) {
            stream.handle.abort();
        }
    }
}

pub(crate) async fn handle_tunnel_open(
    context: &HostContext,
    sink: &SharedWsSink,
    tunnels: &mut HostTunnelTasks,
    streams: &HostTunnelStreams,
    message: WireMessage,
) -> Result<()> {
    let request_id = message.request_id.clone();
    let session_id = message.session_id.clone();
    let payload: TunnelOpenPayload = message.payload_as()?;
    let Some(tunnel_id) = payload.tunnel_id.clone() else {
        send_shared_error(
            sink,
            request_id,
            session_id,
            ErrorCode::InternalError,
            "tunnel.open missing tunnel_id",
        )
        .await?;
        return Ok(());
    };

    let result = validate_host_tunnel_open(&payload).and_then(|_| {
        initial_tunnel_info(&tunnel_id, session_id.clone(), &payload)
            .ok_or_else(|| anyhow!("tunnel.open missing session_id"))
    });
    let Ok(mut info) = result else {
        let reason = result.err().map(|err| err.to_string()).unwrap_or_default();
        let failed = failed_tunnel_info(&tunnel_id, session_id.clone(), &payload, reason.clone());
        send_open_result(sink, request_id, session_id, false, failed).await?;
        return Ok(());
    };

    if payload.direction == TunnelDirection::Remote {
        let bind_addr = format!("{}:{}", payload.listen_addr, payload.listen_port);
        let listener = match TcpListener::bind(&bind_addr).await {
            Ok(listener) => listener,
            Err(err) => {
                let failed =
                    failed_tunnel_info(&tunnel_id, session_id.clone(), &payload, err.to_string());
                send_open_result(sink, request_id, session_id, false, failed).await?;
                return Ok(());
            }
        };
        if let Ok(addr) = listener.local_addr() {
            info.listen_addr = addr.ip().to_string();
            info.listen_port = addr.port();
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let task = spawn_reverse_listener(
            context.clone(),
            sink.clone(),
            ReverseListenerConfig {
                tunnel_id: tunnel_id.clone(),
                session_id: session_id.clone(),
                target_host: payload.target_host.clone(),
                target_port: payload.target_port,
                listener,
                streams: Arc::clone(streams),
                cancel_rx,
            },
        );
        tunnels.insert(
            tunnel_id.clone(),
            HostTunnelTask {
                session_id: session_id.clone(),
                cancel_tx,
                handle: task,
            },
        );
    }

    append_host_audit(
        context,
        "tunnel.opened",
        message.request_id.clone(),
        session_id.clone(),
        Some(format!("{:?}", payload.direction)),
        Some("ok"),
    );
    send_open_result(sink, request_id, session_id, true, info).await
}

pub(crate) async fn handle_tunnel_close(
    context: &HostContext,
    sink: &SharedWsSink,
    tunnels: &mut HostTunnelTasks,
    streams: &HostTunnelStreams,
    message: WireMessage,
) -> Result<()> {
    let request_id = message.request_id.clone();
    let session_id = message.session_id.clone();
    let payload: TunnelClosePayload = message.payload_as()?;
    if let Some(task) = tunnels.remove(&payload.tunnel_id) {
        task.abort();
    }
    let mut streams = streams.lock().await;
    let stream_ids = streams
        .keys()
        .filter(|key| key.starts_with(&payload.tunnel_id))
        .cloned()
        .collect::<Vec<_>>();
    for key in stream_ids {
        if let Some(stream) = streams.remove(&key) {
            stream.handle.abort();
        }
    }
    append_host_audit(
        context,
        "tunnel.closed",
        message.request_id.clone(),
        session_id.clone(),
        Some(payload.tunnel_id.clone()),
        Some("ok"),
    );
    let info = TunnelInfo {
        tunnel_id: payload.tunnel_id,
        session_id: session_id.clone().unwrap_or_default(),
        direction: TunnelDirection::Local,
        listen_addr: String::new(),
        listen_port: 0,
        target_host: String::new(),
        target_port: 0,
        status: TunnelStatus::Closed,
        opened_at: rcw_common::audit::now_rfc3339(),
        last_activity_at: rcw_common::audit::now_rfc3339(),
        idle_timeout_ms: DEFAULT_TUNNEL_IDLE_TIMEOUT_MS,
        bytes_from_listener: 0,
        bytes_from_target: 0,
        active_streams: 0,
        total_streams: 0,
        close_reason: Some("controller_close".to_owned()),
    };
    let mut sink = sink.lock().await;
    send_json(
        &mut sink,
        WireMessage::new(
            TYPE_TUNNEL_CLOSE_RESULT,
            request_id,
            session_id,
            TunnelCloseResultPayload {
                ok: true,
                tunnel: info,
            },
        )?,
    )
    .await
}

pub(crate) async fn handle_tunnel_stream_open(
    context: &HostContext,
    sink: &SharedWsSink,
    streams: &HostTunnelStreams,
    message: WireMessage,
) -> Result<()> {
    let request_id = message.request_id.clone();
    let session_id = message.session_id.clone();
    let payload: TunnelStreamOpenPayload = message.payload_as()?;
    let addr = format!("{}:{}", payload.target_host, payload.target_port);
    let stream = match tokio::time::timeout(TUNNEL_CONNECT_TIMEOUT, TcpStream::connect(&addr)).await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            send_stream_open_result(
                sink,
                request_id,
                session_id,
                &payload.tunnel_id,
                &payload.stream_id,
                false,
                Some(err.to_string()),
            )
            .await?;
            return Ok(());
        }
        Err(err) => {
            send_stream_open_result(
                sink,
                request_id,
                session_id,
                &payload.tunnel_id,
                &payload.stream_id,
                false,
                Some(err.to_string()),
            )
            .await?;
            return Ok(());
        }
    };
    spawn_stream_pump(
        context.clone(),
        sink.clone(),
        streams,
        session_id.clone(),
        payload.tunnel_id.clone(),
        payload.stream_id.clone(),
        stream,
    )
    .await;
    send_stream_open_result(
        sink,
        request_id,
        session_id,
        &payload.tunnel_id,
        &payload.stream_id,
        true,
        None,
    )
    .await
}

pub(crate) async fn handle_tunnel_stream_open_result(
    _context: &HostContext,
    streams: &HostTunnelStreams,
    message: WireMessage,
) -> Result<()> {
    let payload: TunnelStreamOpenResultPayload = message.payload_as()?;
    if !payload.ok {
        let key = stream_key(&payload.tunnel_id, &payload.stream_id);
        let mut streams = streams.lock().await;
        if let Some(stream) = streams.remove(&key) {
            stream.handle.abort();
        }
    }
    Ok(())
}

pub(crate) async fn handle_tunnel_stream_eof(
    streams: &HostTunnelStreams,
    message: WireMessage,
) -> Result<()> {
    let payload: TunnelStreamControlPayload = message.payload_as()?;
    let streams = streams.lock().await;
    if let Some(stream) = streams.get(&stream_key(&payload.tunnel_id, &payload.stream_id)) {
        let _ = stream.write_tx.send(Vec::new()).await;
    }
    Ok(())
}

pub(crate) async fn handle_tunnel_stream_reset(
    streams: &HostTunnelStreams,
    message: WireMessage,
) -> Result<()> {
    let payload: TunnelStreamControlPayload = message.payload_as()?;
    let mut streams = streams.lock().await;
    if let Some(stream) = streams.remove(&stream_key(&payload.tunnel_id, &payload.stream_id)) {
        stream.handle.abort();
    }
    Ok(())
}

pub(crate) async fn handle_tunnel_data(streams: &HostTunnelStreams, bytes: Vec<u8>) -> Result<()> {
    let frame = TunnelDataFrame::decode(&bytes)?;
    let streams = streams.lock().await;
    if let Some(stream) = streams.get(&stream_key(&frame.tunnel_id, &frame.stream_id)) {
        let _ = stream.write_tx.send(frame.payload).await;
    }
    Ok(())
}

fn spawn_reverse_listener(
    context: HostContext,
    sink: SharedWsSink,
    config: ReverseListenerConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ReverseListenerConfig {
            tunnel_id,
            session_id,
            target_host,
            target_port,
            listener,
            streams,
            mut cancel_rx,
        } = config;
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _peer)) = accepted else {
                        continue;
                    };
                    let stream_id = rcw_common::ids::new_stream_id();
                    let request_id = rcw_common::ids::new_request_id();
                    let open = WireMessage::new(
                        TYPE_TUNNEL_STREAM_OPEN,
                        Some(request_id),
                        session_id.clone(),
                        TunnelStreamOpenPayload {
                            tunnel_id: tunnel_id.clone(),
                            stream_id: stream_id.clone(),
                            target_host: target_host.clone(),
                            target_port,
                            source_side: TunnelEndpointSide::Host,
                        },
                    );
                    if let Ok(open) = open {
                        let mut sink_guard = sink.lock().await;
                        if send_json(&mut sink_guard, open).await.is_err() {
                            break;
                        }
                    }
                    spawn_stream_pump(
                        context.clone(),
                        sink.clone(),
                        &streams,
                        session_id.clone(),
                        tunnel_id.clone(),
                        stream_id.clone(),
                        stream,
                    ).await;
                    append_host_audit(
                        &context,
                        "tunnel.stream_open",
                        Some(stream_id),
                        session_id.clone(),
                        Some(tunnel_id.clone()),
                        Some("started"),
                    );
                }
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

async fn spawn_stream_pump(
    context: HostContext,
    sink: SharedWsSink,
    streams: &HostTunnelStreams,
    session_id: Option<String>,
    tunnel_id: String,
    stream_id: String,
    stream: TcpStream,
) {
    let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(64);
    let key = stream_key(&tunnel_id, &stream_id);
    let task_tunnel_id = tunnel_id.clone();
    let task_stream_id = stream_id.clone();
    let task_session_id = session_id.clone();
    let handle = tokio::spawn(async move {
        let (mut reader, mut writer) = stream.into_split();
        let mut read_buffer = vec![0_u8; TUNNEL_COPY_BUFFER_SIZE];
        loop {
            tokio::select! {
                read = reader.read(&mut read_buffer) => {
                    match read {
                        Ok(0) => {
                            let _ = send_stream_control(&sink, task_session_id.clone(), TYPE_TUNNEL_STREAM_EOF, &task_tunnel_id, &task_stream_id, None).await;
                            break;
                        }
                        Ok(n) => {
                            let frame = TunnelDataFrame {
                                tunnel_id: task_tunnel_id.clone(),
                                stream_id: task_stream_id.clone(),
                                payload: read_buffer[..n].to_vec(),
                            };
                            match frame.encode() {
                                Ok(bytes) => {
                                    let mut sink = sink.lock().await;
                                    if sink.send(Message::Binary(bytes)).await.is_err() {
                                        break;
                                    }
                                }
                                Err(err) => {
                                    let _ = send_stream_control(&sink, task_session_id.clone(), TYPE_TUNNEL_STREAM_RESET, &task_tunnel_id, &task_stream_id, Some(err.to_string())).await;
                                    break;
                                }
                            }
                        }
                        Err(err) => {
                            let _ = send_stream_control(&sink, task_session_id.clone(), TYPE_TUNNEL_STREAM_RESET, &task_tunnel_id, &task_stream_id, Some(err.to_string())).await;
                            break;
                        }
                    }
                }
                maybe = write_rx.recv() => {
                    match maybe {
                        Some(bytes) if bytes.is_empty() => {
                            let _ = writer.shutdown().await;
                        }
                        Some(bytes) => {
                            if writer.write_all(&bytes).await.is_err() {
                                let _ = send_stream_control(&sink, task_session_id.clone(), TYPE_TUNNEL_STREAM_RESET, &task_tunnel_id, &task_stream_id, Some("tcp write failed".to_owned())).await;
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        append_host_audit(
            &context,
            "tunnel.stream_closed",
            Some(task_stream_id),
            task_session_id,
            Some(task_tunnel_id),
            Some("closed"),
        );
    });
    let mut streams = streams.lock().await;
    streams.insert(
        key,
        HostTunnelState {
            session_id,
            write_tx,
            handle,
        },
    );
}

async fn send_open_result(
    sink: &SharedWsSink,
    request_id: Option<String>,
    session_id: Option<String>,
    ok: bool,
    tunnel: TunnelInfo,
) -> Result<()> {
    let mut sink = sink.lock().await;
    send_json(
        &mut sink,
        WireMessage::new(
            TYPE_TUNNEL_OPEN_RESULT,
            request_id,
            session_id,
            TunnelOpenResultPayload { ok, tunnel },
        )?,
    )
    .await
}

async fn send_stream_open_result(
    sink: &SharedWsSink,
    request_id: Option<String>,
    session_id: Option<String>,
    tunnel_id: &str,
    stream_id: &str,
    ok: bool,
    message: Option<String>,
) -> Result<()> {
    let mut sink = sink.lock().await;
    send_json(
        &mut sink,
        WireMessage::new(
            TYPE_TUNNEL_STREAM_OPEN_RESULT,
            request_id,
            session_id,
            TunnelStreamOpenResultPayload {
                tunnel_id: tunnel_id.to_owned(),
                stream_id: stream_id.to_owned(),
                ok,
                message,
            },
        )?,
    )
    .await
}

async fn send_stream_control(
    sink: &SharedWsSink,
    session_id: Option<String>,
    kind: &str,
    tunnel_id: &str,
    stream_id: &str,
    reason: Option<String>,
) -> Result<()> {
    let mut sink = sink.lock().await;
    send_json(
        &mut sink,
        WireMessage::new(
            kind,
            Some(rcw_common::ids::new_request_id()),
            session_id,
            TunnelStreamControlPayload {
                tunnel_id: tunnel_id.to_owned(),
                stream_id: stream_id.to_owned(),
                reason,
            },
        )?,
    )
    .await
}

async fn send_shared_error(
    sink: &SharedWsSink,
    request_id: Option<String>,
    session_id: Option<String>,
    code: ErrorCode,
    message: &str,
) -> Result<()> {
    let mut sink = sink.lock().await;
    send_error(&mut sink, request_id, session_id, code, message).await
}

fn validate_host_tunnel_open(payload: &TunnelOpenPayload) -> Result<()> {
    if payload.direction == TunnelDirection::Remote && payload.listen_port == 0 {
        return Err(anyhow!("remote tunnel listen_port must be non-zero"));
    }
    Ok(())
}

fn initial_tunnel_info(
    tunnel_id: &str,
    session_id: Option<String>,
    payload: &TunnelOpenPayload,
) -> Option<TunnelInfo> {
    let now = rcw_common::audit::now_rfc3339();
    Some(TunnelInfo {
        tunnel_id: tunnel_id.to_owned(),
        session_id: session_id?,
        direction: payload.direction,
        listen_addr: payload.listen_addr.clone(),
        listen_port: payload.listen_port,
        target_host: payload.target_host.clone(),
        target_port: payload.target_port,
        status: TunnelStatus::Active,
        opened_at: now.clone(),
        last_activity_at: now,
        idle_timeout_ms: payload.idle_timeout_ms,
        bytes_from_listener: 0,
        bytes_from_target: 0,
        active_streams: 0,
        total_streams: 0,
        close_reason: None,
    })
}

fn failed_tunnel_info(
    tunnel_id: &str,
    session_id: Option<String>,
    payload: &TunnelOpenPayload,
    reason: String,
) -> TunnelInfo {
    let now = rcw_common::audit::now_rfc3339();
    TunnelInfo {
        tunnel_id: tunnel_id.to_owned(),
        session_id: session_id.unwrap_or_default(),
        direction: payload.direction,
        listen_addr: payload.listen_addr.clone(),
        listen_port: payload.listen_port,
        target_host: payload.target_host.clone(),
        target_port: payload.target_port,
        status: TunnelStatus::Failed,
        opened_at: now.clone(),
        last_activity_at: now,
        idle_timeout_ms: payload.idle_timeout_ms,
        bytes_from_listener: 0,
        bytes_from_target: 0,
        active_streams: 0,
        total_streams: 0,
        close_reason: Some(reason),
    }
}

fn stream_key(tunnel_id: &str, stream_id: &str) -> String {
    format!("{tunnel_id}/{stream_id}")
}
