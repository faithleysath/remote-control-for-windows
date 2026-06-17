use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::SinkExt;
use rcw_common::{
    ids::{new_request_id, new_stream_id},
    protocol::{
        ErrorPayload, TunnelClosePayload, TunnelCloseResultPayload, TunnelDirection,
        TunnelEndpointSide, TunnelInfo, TunnelOpenPayload, TunnelOpenResultPayload,
        TunnelStatusPayload, TunnelStatusResultPayload, TunnelStreamControlPayload,
        TunnelStreamOpenPayload, TunnelStreamOpenResultPayload, WireMessage,
        DEFAULT_TUNNEL_IDLE_TIMEOUT_MS, TYPE_ERROR, TYPE_TUNNEL_CLOSE, TYPE_TUNNEL_CLOSE_RESULT,
        TYPE_TUNNEL_OPEN, TYPE_TUNNEL_OPEN_RESULT, TYPE_TUNNEL_STATUS, TYPE_TUNNEL_STATUS_RESULT,
        TYPE_TUNNEL_STREAM_EOF, TYPE_TUNNEL_STREAM_OPEN, TYPE_TUNNEL_STREAM_OPEN_RESULT,
        TYPE_TUNNEL_STREAM_RESET,
    },
    transfer::{BinaryKind, TunnelDataFrame},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Mutex},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    controller_config::{config_wait_timeout, ControllerConfig},
    output::print_json,
    session::SessionStore,
    transport::{
        close_control, connect_control, next_message, send_json, IncomingFrame, WsSink, WsStream,
    },
};

const TUNNEL_COPY_BUFFER_SIZE: usize = 64 * 1024;
const TUNNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TunnelSpec {
    pub(crate) direction: TunnelDirection,
    pub(crate) listen_addr: String,
    pub(crate) listen_port: u16,
    pub(crate) target_host: String,
    pub(crate) target_port: u16,
    pub(crate) idle_timeout_ms: u64,
    pub(crate) allow_non_loopback_listen: bool,
    pub(crate) allow_non_loopback_target: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TunnelOpenResult {
    pub(crate) ok: bool,
    pub(crate) tunnel: TunnelInfo,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TunnelStatusResult {
    pub(crate) ok: bool,
    pub(crate) tunnels: Vec<TunnelInfo>,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct TunnelCloseResult {
    pub(crate) ok: bool,
    pub(crate) tunnel: TunnelInfo,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct ForwardResult {
    pub(crate) ok: bool,
    pub(crate) tunnels: Vec<TunnelInfo>,
}

#[derive(Clone)]
pub(crate) struct TunnelManager {
    inner: Arc<TunnelManagerInner>,
}

struct TunnelManagerInner {
    sink: Mutex<WsSink>,
    streams: Mutex<HashMap<String, ControllerTunnelStream>>,
    local_listeners: Mutex<HashMap<String, JoinHandle<()>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<WireMessage>>>,
    tunnels: Mutex<HashMap<String, String>>,
    session_id: String,
    session_token: String,
}

struct ControllerTunnelStream {
    write_tx: mpsc::Sender<Vec<u8>>,
    handle: JoinHandle<()>,
}

impl TunnelManager {
    pub(crate) async fn connect(
        config: &ControllerConfig,
        store: &dyn SessionStore,
    ) -> Result<Self> {
        let session = store.read_session()?;
        let (sink, stream) = connect_control(&session.server).await?;
        let manager = Self {
            inner: Arc::new(TunnelManagerInner {
                sink: Mutex::new(sink),
                streams: Mutex::new(HashMap::new()),
                local_listeners: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                tunnels: Mutex::new(HashMap::new()),
                session_id: session.session_id.clone(),
                session_token: session.session_token.clone(),
            }),
        };
        manager.spawn_reader(stream);
        let _ = config_wait_timeout(config)?;
        Ok(manager)
    }

    pub(crate) async fn open(
        &self,
        request_id: &str,
        config: &ControllerConfig,
        store: &dyn SessionStore,
        spec: TunnelSpec,
    ) -> Result<TunnelOpenResult> {
        validate_tunnel_spec(&spec)?;
        let session = store.read_session()?;
        let message = WireMessage::new(
            TYPE_TUNNEL_OPEN,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            TunnelOpenPayload {
                session_token: session.session_token.clone(),
                tunnel_id: None,
                direction: spec.direction,
                listen_addr: spec.listen_addr.clone(),
                listen_port: spec.listen_port,
                target_host: spec.target_host.clone(),
                target_port: spec.target_port,
                idle_timeout_ms: spec.idle_timeout_ms,
                allow_non_loopback_listen: spec.allow_non_loopback_listen,
                allow_non_loopback_target: spec.allow_non_loopback_target,
            },
        )?;
        let result = self
            .send_and_wait_for_payload::<TunnelOpenResultPayload>(
                message,
                request_id,
                TYPE_TUNNEL_OPEN_RESULT,
                config_wait_timeout(config)?,
            )
            .await?;
        if !result.ok {
            bail!(
                "tunnel open failed: {}",
                result
                    .tunnel
                    .close_reason
                    .clone()
                    .unwrap_or_else(|| "host rejected tunnel".to_owned())
            );
        }
        if spec.direction == TunnelDirection::Local {
            if let Err(err) = self.spawn_local_listener(result.tunnel.clone()).await {
                let _ = self
                    .send(WireMessage::new(
                        TYPE_TUNNEL_CLOSE,
                        Some(new_request_id()),
                        Some(session.session_id.clone()),
                        TunnelClosePayload {
                            session_token: session.session_token.clone(),
                            tunnel_id: result.tunnel.tunnel_id.clone(),
                        },
                    )?)
                    .await;
                return Err(err);
            }
        }
        self.inner.tunnels.lock().await.insert(
            result.tunnel.tunnel_id.clone(),
            session.session_token.clone(),
        );
        store.touch_session(session)?;
        Ok(TunnelOpenResult {
            ok: true,
            tunnel: result.tunnel,
            request_id: request_id.to_owned(),
        })
    }

    pub(crate) async fn status(
        config: &ControllerConfig,
        store: &dyn SessionStore,
        request_id: &str,
        tunnel_id: Option<String>,
    ) -> Result<TunnelStatusResult> {
        let session = store.read_session()?;
        let message = WireMessage::new(
            TYPE_TUNNEL_STATUS,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            TunnelStatusPayload {
                session_token: session.session_token.clone(),
                tunnel_id,
            },
        )?;
        let (mut sink, mut stream) = connect_control(&session.server).await?;
        send_json(&mut sink, message).await?;
        let result = loop {
            match next_message(&mut stream, Some(config_wait_timeout(config)?)).await? {
                IncomingFrame::Text(message) if message.kind == TYPE_TUNNEL_STATUS_RESULT => {
                    let payload: TunnelStatusResultPayload = message.payload_as()?;
                    break payload;
                }
                IncomingFrame::Text(message) if message.kind == TYPE_ERROR => {
                    bail_error_payload(message.payload_as()?)?;
                }
                _ => {}
            }
        };
        close_control(&mut sink).await;
        store.touch_session(session)?;
        Ok(TunnelStatusResult {
            ok: result.ok,
            tunnels: result.tunnels,
            request_id: request_id.to_owned(),
        })
    }

    pub(crate) async fn close(
        config: &ControllerConfig,
        store: &dyn SessionStore,
        request_id: &str,
        tunnel_id: &str,
    ) -> Result<TunnelCloseResult> {
        let session = store.read_session()?;
        let message = WireMessage::new(
            TYPE_TUNNEL_CLOSE,
            Some(request_id.to_owned()),
            Some(session.session_id.clone()),
            TunnelClosePayload {
                session_token: session.session_token.clone(),
                tunnel_id: tunnel_id.to_owned(),
            },
        )?;
        let (mut sink, mut stream) = connect_control(&session.server).await?;
        send_json(&mut sink, message).await?;
        let result = loop {
            match next_message(&mut stream, Some(config_wait_timeout(config)?)).await? {
                IncomingFrame::Text(message) if message.kind == TYPE_TUNNEL_CLOSE_RESULT => {
                    let payload: TunnelCloseResultPayload = message.payload_as()?;
                    break payload;
                }
                IncomingFrame::Text(message) if message.kind == TYPE_ERROR => {
                    bail_error_payload(message.payload_as()?)?;
                }
                _ => {}
            }
        };
        close_control(&mut sink).await;
        store.touch_session(session)?;
        Ok(TunnelCloseResult {
            ok: result.ok,
            tunnel: result.tunnel,
            request_id: request_id.to_owned(),
        })
    }

    pub(crate) async fn close_owned(
        &self,
        config: &ControllerConfig,
        store: &dyn SessionStore,
        request_id: &str,
        tunnel_id: &str,
    ) -> Result<TunnelCloseResult> {
        let result = Self::close(config, store, request_id, tunnel_id).await?;
        self.cleanup_tunnel_local(tunnel_id).await;
        self.inner.tunnels.lock().await.remove(tunnel_id);
        Ok(result)
    }

    pub(crate) async fn shutdown(&self) {
        let listener_ids = {
            let listeners = self.inner.local_listeners.lock().await;
            listeners.keys().cloned().collect::<Vec<_>>()
        };
        for tunnel_id in listener_ids {
            self.abort_local_listener(&tunnel_id).await;
        }
        let keys = {
            let streams = self.inner.streams.lock().await;
            streams.keys().cloned().collect::<Vec<_>>()
        };
        let mut streams = self.inner.streams.lock().await;
        let streams_to_abort = keys
            .into_iter()
            .filter_map(|key| streams.remove(&key))
            .collect::<Vec<_>>();
        drop(streams);
        for stream in streams_to_abort {
            stream.handle.abort();
            let _ = stream.handle.await;
        }
        let tunnel_ids = {
            let tunnels = self.inner.tunnels.lock().await;
            tunnels.keys().cloned().collect::<Vec<_>>()
        };
        for tunnel_id in tunnel_ids {
            let _ = self
                .send(
                    WireMessage::new(
                        TYPE_TUNNEL_CLOSE,
                        Some(new_request_id()),
                        Some(self.inner.session_id.clone()),
                        TunnelClosePayload {
                            session_token: self.inner.session_token.clone(),
                            tunnel_id: tunnel_id.clone(),
                        },
                    )
                    .expect("tunnel close serializes"),
                )
                .await;
            self.cleanup_tunnel_streams(&tunnel_id).await;
        }
        self.inner.tunnels.lock().await.clear();
        let mut sink = self.inner.sink.lock().await;
        close_control(&mut sink).await;
    }

    async fn spawn_local_listener(&self, tunnel: TunnelInfo) -> Result<()> {
        let bind_addr = format!("{}:{}", tunnel.listen_addr, tunnel.listen_port);
        let listener = TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("failed to bind local tunnel listener {bind_addr}"))?;
        let manager = self.clone();
        let tunnel_id = tunnel.tunnel_id.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let stream_id = new_stream_id();
                let request_id = new_request_id();
                let open = WireMessage::new(
                    TYPE_TUNNEL_STREAM_OPEN,
                    Some(request_id),
                    Some(tunnel.session_id.clone()),
                    TunnelStreamOpenPayload {
                        tunnel_id: tunnel.tunnel_id.clone(),
                        stream_id: stream_id.clone(),
                        target_host: tunnel.target_host.clone(),
                        target_port: tunnel.target_port,
                        source_side: TunnelEndpointSide::Controller,
                    },
                );
                if let Ok(open) = open {
                    if manager.send(open).await.is_err() {
                        break;
                    }
                }
                manager
                    .spawn_stream_pump(tunnel.tunnel_id.clone(), stream_id, stream)
                    .await;
            }
        });
        self.inner
            .local_listeners
            .lock()
            .await
            .insert(tunnel_id, handle);
        Ok(())
    }

    async fn spawn_stream_pump(&self, tunnel_id: String, stream_id: String, stream: TcpStream) {
        let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(64);
        let key = stream_key(&tunnel_id, &stream_id);
        let manager = self.clone();
        let handle = tokio::spawn(async move {
            let (mut reader, mut writer) = stream.into_split();
            let mut read_buffer = vec![0_u8; TUNNEL_COPY_BUFFER_SIZE];
            loop {
                tokio::select! {
                    read = reader.read(&mut read_buffer) => {
                        match read {
                            Ok(0) => {
                                let _ = manager
                                    .send_stream_control(TYPE_TUNNEL_STREAM_EOF, &tunnel_id, &stream_id, None)
                                    .await;
                                break;
                            }
                            Ok(n) => {
                                let frame = TunnelDataFrame {
                                    tunnel_id: tunnel_id.clone(),
                                    stream_id: stream_id.clone(),
                                    payload: read_buffer[..n].to_vec(),
                                };
                                match frame.encode() {
                                    Ok(bytes) => {
                                        let mut sink = manager.inner.sink.lock().await;
                                        if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(err) => {
                                        let _ = manager
                                            .send_stream_control(
                                                TYPE_TUNNEL_STREAM_RESET,
                                                &tunnel_id,
                                                &stream_id,
                                                Some(err.to_string()),
                                            )
                                            .await;
                                        break;
                                    }
                                }
                            }
                            Err(err) => {
                                let _ = manager
                                    .send_stream_control(
                                        TYPE_TUNNEL_STREAM_RESET,
                                        &tunnel_id,
                                        &stream_id,
                                        Some(err.to_string()),
                                    )
                                    .await;
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
                                    let _ = manager
                                        .send_stream_control(
                                            TYPE_TUNNEL_STREAM_RESET,
                                            &tunnel_id,
                                            &stream_id,
                                            Some("tcp write failed".to_owned()),
                                        )
                                        .await;
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });
        let mut streams = self.inner.streams.lock().await;
        streams.insert(key, ControllerTunnelStream { write_tx, handle });
    }

    fn spawn_reader(&self, mut stream: WsStream) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                let frame = next_message(&mut stream, None).await;
                let Ok(frame) = frame else {
                    break;
                };
                if manager.handle_incoming(frame).await.is_err() {
                    break;
                }
            }
        });
    }

    async fn handle_incoming(&self, frame: IncomingFrame) -> Result<()> {
        match frame {
            IncomingFrame::Text(message) => {
                if let Some(request_id) = message.request_id.clone() {
                    if let Some(tx) = self.inner.pending.lock().await.remove(&request_id) {
                        let _ = tx.send(message);
                        return Ok(());
                    }
                }
                match message.kind.as_str() {
                    TYPE_TUNNEL_STREAM_OPEN => self.handle_stream_open(message).await,
                    TYPE_TUNNEL_STREAM_OPEN_RESULT => self.handle_stream_open_result(message).await,
                    TYPE_TUNNEL_STREAM_EOF => self.handle_stream_eof(message).await,
                    TYPE_TUNNEL_STREAM_RESET => self.handle_stream_reset(message).await,
                    TYPE_TUNNEL_CLOSE_RESULT => Ok(()),
                    TYPE_ERROR => {
                        let error: ErrorPayload = message.payload_as()?;
                        Err(anyhow!("{:?}: {}", error.code, error.message))
                    }
                    _ => Ok(()),
                }
            }
            IncomingFrame::Binary(bytes) => {
                if bytes.first().copied() != Some(BinaryKind::TunnelData as u8) {
                    return Ok(());
                }
                let frame = TunnelDataFrame::decode(&bytes)?;
                if let Some(stream) = self
                    .inner
                    .streams
                    .lock()
                    .await
                    .get(&stream_key(&frame.tunnel_id, &frame.stream_id))
                {
                    let _ = stream.write_tx.send(frame.payload).await;
                }
                Ok(())
            }
        }
    }

    async fn handle_stream_open(&self, message: WireMessage) -> Result<()> {
        let request_id = message.request_id.clone();
        let session_id = message.session_id.clone();
        let payload: TunnelStreamOpenPayload = message.payload_as()?;
        let addr = format!("{}:{}", payload.target_host, payload.target_port);
        let stream =
            match tokio::time::timeout(TUNNEL_CONNECT_TIMEOUT, TcpStream::connect(&addr)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(err)) => {
                    self.send_stream_open_result(
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
                    self.send_stream_open_result(
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
        self.spawn_stream_pump(payload.tunnel_id.clone(), payload.stream_id.clone(), stream)
            .await;
        self.send_stream_open_result(
            request_id,
            session_id,
            &payload.tunnel_id,
            &payload.stream_id,
            true,
            None,
        )
        .await
    }

    async fn handle_stream_open_result(&self, message: WireMessage) -> Result<()> {
        let payload: TunnelStreamOpenResultPayload = message.payload_as()?;
        if !payload.ok {
            self.remove_stream(&payload.tunnel_id, &payload.stream_id)
                .await;
        }
        Ok(())
    }

    async fn handle_stream_eof(&self, message: WireMessage) -> Result<()> {
        let payload: TunnelStreamControlPayload = message.payload_as()?;
        if let Some(stream) = self
            .inner
            .streams
            .lock()
            .await
            .get(&stream_key(&payload.tunnel_id, &payload.stream_id))
        {
            let _ = stream.write_tx.send(Vec::new()).await;
        }
        Ok(())
    }

    async fn handle_stream_reset(&self, message: WireMessage) -> Result<()> {
        let payload: TunnelStreamControlPayload = message.payload_as()?;
        self.remove_stream(&payload.tunnel_id, &payload.stream_id)
            .await;
        Ok(())
    }

    async fn remove_stream(&self, tunnel_id: &str, stream_id: &str) {
        let mut streams = self.inner.streams.lock().await;
        let stream = streams.remove(&stream_key(tunnel_id, stream_id));
        drop(streams);
        if let Some(stream) = stream {
            stream.handle.abort();
            let _ = stream.handle.await;
        }
    }

    async fn cleanup_tunnel_local(&self, tunnel_id: &str) {
        self.abort_local_listener(tunnel_id).await;
        self.cleanup_tunnel_streams(tunnel_id).await;
    }

    async fn abort_local_listener(&self, tunnel_id: &str) {
        let mut listeners = self.inner.local_listeners.lock().await;
        let handle = listeners.remove(tunnel_id);
        drop(listeners);
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
    }

    async fn cleanup_tunnel_streams(&self, tunnel_id: &str) {
        let mut streams = self.inner.streams.lock().await;
        let keys = streams
            .keys()
            .filter(|key| stream_tunnel_id(key) == tunnel_id)
            .cloned()
            .collect::<Vec<_>>();
        let streams_to_abort = keys
            .into_iter()
            .filter_map(|key| streams.remove(&key))
            .collect::<Vec<_>>();
        drop(streams);
        for stream in streams_to_abort {
            stream.handle.abort();
            let _ = stream.handle.await;
        }
    }

    async fn send(&self, message: WireMessage) -> Result<()> {
        let mut sink = self.inner.sink.lock().await;
        send_json(&mut sink, message).await
    }

    async fn send_stream_open_result(
        &self,
        request_id: Option<String>,
        session_id: Option<String>,
        tunnel_id: &str,
        stream_id: &str,
        ok: bool,
        message: Option<String>,
    ) -> Result<()> {
        self.send(WireMessage::new(
            TYPE_TUNNEL_STREAM_OPEN_RESULT,
            request_id,
            session_id,
            TunnelStreamOpenResultPayload {
                tunnel_id: tunnel_id.to_owned(),
                stream_id: stream_id.to_owned(),
                ok,
                message,
            },
        )?)
        .await
    }

    async fn send_stream_control(
        &self,
        kind: &str,
        tunnel_id: &str,
        stream_id: &str,
        reason: Option<String>,
    ) -> Result<()> {
        self.send(WireMessage::new(
            kind,
            Some(new_request_id()),
            Some(self.inner.session_id.clone()),
            TunnelStreamControlPayload {
                tunnel_id: tunnel_id.to_owned(),
                stream_id: stream_id.to_owned(),
                reason,
            },
        )?)
        .await
    }

    async fn send_and_wait_for_payload<T>(
        &self,
        message: WireMessage,
        request_id: &str,
        kind: &str,
        wait: Duration,
    ) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .await
            .insert(request_id.to_owned(), tx);
        if let Err(err) = self.send(message).await {
            self.inner.pending.lock().await.remove(request_id);
            return Err(err);
        }
        let message = match tokio::time::timeout(wait, rx).await {
            Ok(Ok(message)) => message,
            Ok(Err(_)) => bail!("tunnel manager stopped before {kind}"),
            Err(_) => {
                self.inner.pending.lock().await.remove(request_id);
                bail!("timed out waiting for {kind}");
            }
        };
        if message.kind == TYPE_ERROR {
            bail_error_payload(message.payload_as()?)?;
        }
        if message.kind != kind {
            bail!("expected {kind}, got {}", message.kind);
        }
        Ok(message.payload_as()?)
    }
}

pub(crate) async fn forward_cli(
    config: &ControllerConfig,
    store: &dyn SessionStore,
    specs: Vec<TunnelSpec>,
    json: bool,
) -> Result<i32> {
    if specs.is_empty() {
        bail!("forward requires at least one -L or -R mapping");
    }
    let manager = TunnelManager::connect(config, store).await?;
    let mut tunnels = Vec::new();
    for spec in specs {
        let request_id = new_request_id();
        let result = manager.open(&request_id, config, store, spec).await?;
        tunnels.push(result.tunnel);
    }
    let output = ForwardResult { ok: true, tunnels };
    if json {
        print_json(serde_json::to_value(&output)?)?;
    } else {
        for tunnel in &output.tunnels {
            println!(
                "{} {:?} {}:{} -> {}:{}",
                tunnel.tunnel_id,
                tunnel.direction,
                tunnel.listen_addr,
                tunnel.listen_port,
                tunnel.target_host,
                tunnel.target_port
            );
        }
        println!("forwarding active; press Ctrl-C to close");
    }
    tokio::signal::ctrl_c().await?;
    manager.shutdown().await;
    Ok(0)
}

pub(crate) fn parse_forward_spec(value: &str, direction: TunnelDirection) -> Result<TunnelSpec> {
    let (listen, target) = value
        .split_once('=')
        .ok_or_else(|| anyhow!("forward spec must be listen_host:port=target_host:port"))?;
    let (listen_addr, listen_port) = parse_host_port(listen)?;
    let (target_host, target_port) = parse_host_port(target)?;
    Ok(TunnelSpec {
        direction,
        listen_addr,
        listen_port,
        target_host,
        target_port,
        idle_timeout_ms: DEFAULT_TUNNEL_IDLE_TIMEOUT_MS,
        allow_non_loopback_listen: false,
        allow_non_loopback_target: false,
    })
}

fn parse_host_port(value: &str) -> Result<(String, u16)> {
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("address must be host:port"))?;
    let port = port
        .parse::<u16>()
        .with_context(|| format!("invalid port in {value:?}"))?;
    if host.trim().is_empty() || port == 0 {
        bail!("address must include non-empty host and non-zero port");
    }
    Ok((host.to_owned(), port))
}

fn validate_tunnel_spec(spec: &TunnelSpec) -> Result<()> {
    if spec.listen_port == 0 || spec.target_port == 0 {
        bail!("tunnel ports must be non-zero");
    }
    if !spec.allow_non_loopback_listen && !is_loopback_name(&spec.listen_addr) {
        bail!("tunnel listen address must be loopback unless explicitly allowed");
    }
    if !spec.allow_non_loopback_target && !is_loopback_name(&spec.target_host) {
        bail!("tunnel target host must be loopback unless explicitly allowed");
    }
    Ok(())
}

fn is_loopback_name(value: &str) -> bool {
    value.eq_ignore_ascii_case("localhost")
        || value == "127.0.0.1"
        || value == "::1"
        || value.starts_with("127.")
}

fn bail_error_payload(error: ErrorPayload) -> Result<()> {
    bail!("{:?}: {}", error.code, error.message)
}

fn stream_key(tunnel_id: &str, stream_id: &str) -> String {
    format!("{tunnel_id}/{stream_id}")
}

fn stream_tunnel_id(key: &str) -> &str {
    key.split_once('/').map_or(key, |(tunnel_id, _)| tunnel_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        controller_config::ControllerConfig,
        session::{MemorySessionStore, SessionFile, SessionStore},
    };
    use futures_util::StreamExt;
    use rcw_common::protocol::TunnelStatus;
    use tokio::net::TcpListener as TokioTcpListener;
    use tokio_tungstenite::accept_async;

    #[test]
    fn parses_forward_spec() {
        let spec =
            parse_forward_spec("127.0.0.1:15432=127.0.0.1:5432", TunnelDirection::Local).unwrap();
        assert_eq!(spec.listen_addr, "127.0.0.1");
        assert_eq!(spec.listen_port, 15432);
        assert_eq!(spec.target_host, "127.0.0.1");
        assert_eq!(spec.target_port, 5432);
    }

    #[tokio::test]
    async fn shutdown_releases_local_listener_port() {
        let ws_listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = ws_listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            let (stream, _) = ws_listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();
            while websocket.next().await.is_some() {}
        });
        let session = SessionFile {
            server: format!("http://{server_addr}"),
            machine_id: "machine-test".to_owned(),
            session_id: "session-test".to_owned(),
            session_token: "token-test".to_owned(),
            created_at: "2026-06-15T00:00:00Z".to_owned(),
            last_used_at: "2026-06-15T00:00:00Z".to_owned(),
        };
        let store = MemorySessionStore::default();
        store.write_session(&session).unwrap();
        let config = ControllerConfig {
            server: None,
            token: None,
            audit_label: None,
        };
        let manager = TunnelManager::connect(&config, &store).await.unwrap();
        let port_probe = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_port = port_probe.local_addr().unwrap().port();
        drop(port_probe);
        let tunnel = TunnelInfo {
            tunnel_id: "tunnel-test".to_owned(),
            session_id: session.session_id,
            direction: TunnelDirection::Local,
            listen_addr: "127.0.0.1".to_owned(),
            listen_port,
            target_host: "127.0.0.1".to_owned(),
            target_port: 22,
            status: TunnelStatus::Active,
            opened_at: "2026-06-15T00:00:00Z".to_owned(),
            last_activity_at: "2026-06-15T00:00:00Z".to_owned(),
            idle_timeout_ms: DEFAULT_TUNNEL_IDLE_TIMEOUT_MS,
            bytes_from_listener: 0,
            bytes_from_target: 0,
            active_streams: 0,
            total_streams: 0,
            close_reason: None,
        };
        manager.spawn_local_listener(tunnel.clone()).await.unwrap();
        manager
            .inner
            .tunnels
            .lock()
            .await
            .insert(tunnel.tunnel_id.clone(), "token-test".to_owned());

        manager.shutdown().await;

        let rebound = TokioTcpListener::bind(("127.0.0.1", listen_port)).await;
        assert!(
            rebound.is_ok(),
            "local tunnel listener port was not released: {:?}",
            rebound.err()
        );
    }
}
