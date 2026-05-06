use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::Engine;
use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, read_preface, write_frame};
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectRequest, PortConnectResponse, PortConnectionCloseRequest,
    PortConnectionReadRequest, PortConnectionReadResponse, PortConnectionWriteRequest,
    PortForwardLease, PortForwardProtocol, PortLeaseRenewRequest, PortListenAcceptRequest,
    PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest, PortListenResponse,
    PortUdpDatagramReadRequest, PortUdpDatagramReadResponse, PortUdpDatagramWriteRequest,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{AppState, HostRpcError};

const READ_BUF_SIZE: usize = 64 * 1024;
const EXPIRED_FORWARD_SWEEP_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Default)]
pub struct PortForwardState {
    tcp_listeners: Arc<Mutex<HashMap<String, Arc<TcpListenerEntry>>>>,
    udp_sockets: Arc<Mutex<HashMap<String, Arc<UdpSocketEntry>>>>,
    tcp_connections: Arc<Mutex<HashMap<String, Arc<TcpConnection>>>>,
    leases: Arc<Mutex<HashMap<String, LeaseEntry>>>,
}

struct TcpListenerEntry {
    listener: TcpListener,
    cancel: CancellationToken,
    lease_id: Option<String>,
}

struct UdpSocketEntry {
    socket: UdpSocket,
    cancel: CancellationToken,
    lease_id: Option<String>,
}

struct TcpConnection {
    reader: Mutex<OwnedReadHalf>,
    writer: Mutex<OwnedWriteHalf>,
    cancel: CancellationToken,
    lease_id: Option<String>,
}

struct LeaseEntry {
    expires_at: Instant,
    binds: HashSet<String>,
    connections: HashSet<String>,
}

struct TunnelState {
    target: String,
    cancel: CancellationToken,
    tx: mpsc::Sender<Frame>,
    tcp_writers: Mutex<HashMap<u32, Arc<Mutex<OwnedWriteHalf>>>>,
    udp_sockets: Mutex<HashMap<u32, Arc<UdpSocket>>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
    next_daemon_stream_id: AtomicU32,
}

#[derive(Debug, Deserialize)]
struct EndpointMeta {
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct EndpointOkMeta {
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct TcpAcceptMeta {
    listener_stream_id: u32,
    peer: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UdpDatagramMeta {
    peer: String,
}

#[derive(Debug, Serialize)]
struct ErrorMeta {
    code: String,
    message: String,
    fatal: bool,
}

impl TcpConnection {
    fn new(stream: TcpStream, lease_id: Option<String>) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            cancel: CancellationToken::new(),
            lease_id,
        }
    }
}

pub async fn serve_tunnel<S>(state: Arc<AppState>, stream: S) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    read_preface(&mut reader)
        .await
        .map_err(|err| rpc_error("invalid_port_tunnel", err.to_string()))?;

    let (tx, mut rx) = mpsc::channel::<Frame>(128);
    let tunnel = Arc::new(TunnelState {
        target: state.config.target.clone(),
        cancel: state.shutdown.child_token(),
        tx: tx.clone(),
        tcp_writers: Mutex::new(HashMap::new()),
        udp_sockets: Mutex::new(HashMap::new()),
        stream_cancels: Mutex::new(HashMap::new()),
        next_daemon_stream_id: AtomicU32::new(2),
    });
    let writer_cancel = tunnel.cancel.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_cancel.cancelled() => return,
                frame = rx.recv() => {
                    let Some(frame) = frame else {
                        return;
                    };
                    if write_frame(&mut writer, &frame).await.is_err() {
                        writer_cancel.cancel();
                        return;
                    }
                }
            }
        }
    });

    let result = tunnel_read_loop(tunnel.clone(), &mut reader).await;
    tunnel.cancel.cancel();
    drop(tx);
    writer_task.abort();
    result
}

async fn tunnel_read_loop<R>(tunnel: Arc<TunnelState>, reader: &mut R) -> Result<(), HostRpcError>
where
    R: AsyncRead + Unpin,
{
    loop {
        let frame = tokio::select! {
            _ = tunnel.cancel.cancelled() => return Ok(()),
            frame = read_frame(reader) => {
                match frame {
                    Ok(frame) => frame,
                    Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(()),
                    Err(err) => {
                        let _ = send_tunnel_error(&tunnel, 0, "invalid_port_tunnel", err.to_string(), true)
                            .await;
                        return Err(rpc_error("invalid_port_tunnel", err.to_string()));
                    }
                }
            }
        };

        let stream_id = frame.stream_id;
        if let Err(err) = handle_tunnel_frame(tunnel.clone(), frame).await {
            let _ =
                send_tunnel_error(&tunnel, stream_id, err.code, err.message.clone(), false).await;
        }
    }
}

async fn handle_tunnel_frame(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    match frame.frame_type {
        FrameType::TcpListen => tunnel_tcp_listen(tunnel, frame).await,
        FrameType::TcpConnect => tunnel_tcp_connect(tunnel, frame).await,
        FrameType::TcpData => tunnel_tcp_data(&tunnel, frame.stream_id, &frame.data).await,
        FrameType::TcpEof => tunnel_tcp_eof(&tunnel, frame.stream_id).await,
        FrameType::Close => tunnel_close_stream(&tunnel, frame.stream_id).await,
        FrameType::UdpBind => tunnel_udp_bind(tunnel, frame).await,
        FrameType::UdpDatagram => tunnel_udp_datagram(&tunnel, frame).await,
        _ => Err(rpc_error(
            "invalid_port_tunnel",
            format!("unexpected frame type `{:?}` from broker", frame.frame_type),
        )),
    }
}

async fn tunnel_tcp_listen(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let listener = TcpListener::bind(&endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpListenOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await?;

    tracing::info!(
        target = %tunnel.target,
        stream_id = frame.stream_id,
        endpoint = %bound_endpoint,
        "opened port tunnel tcp listener"
    );
    tokio::spawn(tunnel_tcp_accept_loop(
        tunnel,
        frame.stream_id,
        listener,
        stream_cancel,
    ));
    Ok(())
}

async fn tunnel_tcp_accept_loop(
    tunnel: Arc<TunnelState>,
    listener_stream_id: u32,
    listener: TcpListener,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    listener_stream_id,
                    "port_accept_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let stream_id = tunnel.next_daemon_stream_id.fetch_add(2, Ordering::Relaxed);
        let (reader, writer) = stream.into_split();
        tunnel
            .tcp_writers
            .lock()
            .await
            .insert(stream_id, Arc::new(Mutex::new(writer)));
        let stream_cancel = tunnel.cancel.child_token();
        tunnel
            .stream_cancels
            .lock()
            .await
            .insert(stream_id, stream_cancel.clone());
        if tunnel
            .send(Frame {
                frame_type: FrameType::TcpAccept,
                flags: 0,
                stream_id,
                meta: match encode_frame_meta(&TcpAcceptMeta {
                    listener_stream_id,
                    peer: peer.to_string(),
                }) {
                    Ok(meta) => meta,
                    Err(err) => {
                        let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false)
                            .await;
                        continue;
                    }
                },
                data: Vec::new(),
            })
            .await
            .is_err()
        {
            return;
        }
        tokio::spawn(tunnel_tcp_read_loop(
            tunnel.clone(),
            stream_id,
            reader,
            stream_cancel,
        ));
    }
}

async fn tunnel_tcp_connect(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let stream = TcpStream::connect(endpoint.as_str())
        .await
        .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?;
    let (reader, writer) = stream.into_split();
    tunnel
        .tcp_writers
        .lock()
        .await
        .insert(frame.stream_id, Arc::new(Mutex::new(writer)));
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpConnectOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_tcp_read_loop(
        tunnel,
        frame.stream_id,
        reader,
        stream_cancel,
    ));
    Ok(())
}

async fn tunnel_tcp_read_loop(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => return,
            read = reader.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ = tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpEof,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                return;
            }
            Ok(read) => {
                if tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: buf[..read].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        }
    }
}

async fn tunnel_tcp_data(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
    data: &[u8],
) -> Result<(), HostRpcError> {
    let writer = tunnel
        .tcp_writers
        .lock()
        .await
        .get(&stream_id)
        .cloned()
        .ok_or_else(|| {
            rpc_error(
                "unknown_port_connection",
                format!("unknown tunnel tcp stream `{stream_id}`"),
            )
        })?;
    writer
        .lock()
        .await
        .write_all(data)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))
}

async fn tunnel_tcp_eof(tunnel: &Arc<TunnelState>, stream_id: u32) -> Result<(), HostRpcError> {
    if let Some(writer) = tunnel.tcp_writers.lock().await.get(&stream_id).cloned() {
        writer
            .lock()
            .await
            .shutdown()
            .await
            .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    }
    Ok(())
}

async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    if let Some(cancel) = tunnel.stream_cancels.lock().await.remove(&stream_id) {
        cancel.cancel();
    }
    tunnel.tcp_writers.lock().await.remove(&stream_id);
    tunnel.udp_sockets.lock().await.remove(&stream_id);
    Ok(())
}

async fn tunnel_udp_bind(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let socket = Arc::new(
        UdpSocket::bind(&endpoint)
            .await
            .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?,
    );
    let bound_endpoint = socket
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    tunnel
        .udp_sockets
        .lock()
        .await
        .insert(frame.stream_id, socket.clone());
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::UdpBindOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint,
            })?,
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_udp_read_loop(
        tunnel,
        frame.stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}

async fn tunnel_udp_read_loop(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => return,
            received = socket.recv_from(&mut buf) => received,
        };
        let (read, peer) = match received {
            Ok(received) => received,
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let meta = match encode_frame_meta(&UdpDatagramMeta {
            peer: peer.to_string(),
        }) {
            Ok(meta) => meta,
            Err(err) => {
                let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false).await;
                return;
            }
        };
        if tunnel
            .send(Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id,
                meta,
                data: buf[..read].to_vec(),
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn tunnel_udp_datagram(tunnel: &Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: UdpDatagramMeta = decode_frame_meta(&frame)?;
    let socket = tunnel
        .udp_sockets
        .lock()
        .await
        .get(&frame.stream_id)
        .cloned()
        .ok_or_else(|| {
            rpc_error(
                "unknown_port_bind",
                format!("unknown tunnel udp stream `{}`", frame.stream_id),
            )
        })?;
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    Ok(())
}

impl TunnelState {
    async fn send(&self, frame: Frame) -> Result<(), HostRpcError> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| rpc_error("port_tunnel_closed", "port tunnel writer is closed"))
    }
}

async fn send_tunnel_error(
    tunnel: &TunnelState,
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    let meta = encode_frame_meta(&ErrorMeta {
        code: code.into(),
        message: message.into(),
        fatal,
    })?;
    tunnel
        .send(Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id,
            meta,
            data: Vec::new(),
        })
        .await
}

fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, HostRpcError> {
    serde_json::from_slice(&frame.meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, HostRpcError> {
    serde_json::to_vec(meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

pub async fn shutdown_local(state: &AppState) {
    state.shutdown.cancel();

    let listeners = state
        .port_forwards
        .tcp_listeners
        .lock()
        .await
        .drain()
        .map(|(_, listener)| listener)
        .collect::<Vec<_>>();
    let sockets = state
        .port_forwards
        .udp_sockets
        .lock()
        .await
        .drain()
        .map(|(_, socket)| socket)
        .collect::<Vec<_>>();
    let connections = state
        .port_forwards
        .tcp_connections
        .lock()
        .await
        .drain()
        .map(|(_, connection)| connection)
        .collect::<Vec<_>>();
    state.port_forwards.leases.lock().await.clear();

    for listener in listeners {
        listener.cancel.cancel();
    }
    for socket in sockets {
        socket.cancel.cancel();
    }
    for connection in connections {
        connection.cancel.cancel();
    }
}

pub async fn listen_local(
    state: Arc<AppState>,
    req: PortListenRequest,
) -> Result<PortListenResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let endpoint = normalize_endpoint(&req.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    match req.protocol {
        PortForwardProtocol::Tcp => listen_tcp(state, &endpoint, req.lease).await,
        PortForwardProtocol::Udp => listen_udp(state, &endpoint, req.lease).await,
    }
}

pub async fn listen_accept_local(
    state: Arc<AppState>,
    req: PortListenAcceptRequest,
) -> Result<PortListenAcceptResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let listener = tcp_listener(&state, &req.bind_id).await?;
    let shutdown = state.shutdown.clone();
    let (stream, peer_addr) = tokio::select! {
        _ = shutdown.cancelled() => {
            return Err(port_bind_closed(&req.bind_id));
        }
        _ = listener.cancel.cancelled() => {
            return Err(port_bind_closed(&req.bind_id));
        }
        accepted = listener.listener.accept() => accepted
            .map_err(|err| {
                if listener.cancel.is_cancelled() {
                    port_bind_closed(&req.bind_id)
                } else {
                    rpc_error("port_accept_failed", err.to_string())
                }
            })?,
    };
    if listener.cancel.is_cancelled() || state.shutdown.is_cancelled() {
        return Err(port_bind_closed(&req.bind_id));
    }
    let connection_id = format!("conn_{}", uuid::Uuid::new_v4().simple());
    state.port_forwards.tcp_connections.lock().await.insert(
        connection_id.clone(),
        Arc::new(TcpConnection::new(stream, listener.lease_id.clone())),
    );
    if let Some(lease_id) = &listener.lease_id {
        track_connection_lease(&state.port_forwards, lease_id, &connection_id).await;
    }
    tracing::debug!(
        target = %state.config.target,
        bind_id = %req.bind_id,
        connection_id = %connection_id,
        peer = %peer_addr,
        "accepted port forward tcp connection"
    );
    Ok(PortListenAcceptResponse { connection_id })
}

pub async fn listen_close_local(
    state: Arc<AppState>,
    req: PortListenCloseRequest,
) -> Result<EmptyResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let tcp_listener = state
        .port_forwards
        .tcp_listeners
        .lock()
        .await
        .remove(&req.bind_id);
    let udp_socket = state
        .port_forwards
        .udp_sockets
        .lock()
        .await
        .remove(&req.bind_id);
    if let Some(listener) = tcp_listener {
        if let Some(lease_id) = &listener.lease_id {
            untrack_bind_lease(&state.port_forwards, lease_id, &req.bind_id).await;
        }
        listener.cancel.cancel();
    }
    if let Some(socket) = udp_socket {
        if let Some(lease_id) = &socket.lease_id {
            untrack_bind_lease(&state.port_forwards, lease_id, &req.bind_id).await;
        }
        socket.cancel.cancel();
    }
    tracing::debug!(
        target = %state.config.target,
        bind_id = %req.bind_id,
        "closed port forward bind"
    );
    Ok(EmptyResponse {})
}

pub async fn connect_local(
    state: Arc<AppState>,
    req: PortConnectRequest,
) -> Result<PortConnectResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    match req.protocol {
        PortForwardProtocol::Tcp => connect_tcp(state, &req.endpoint, req.lease).await,
        PortForwardProtocol::Udp => Err(rpc_error(
            "unsupported_operation",
            "udp connect is not used by this forwarding protocol",
        )),
    }
}

pub async fn connection_read_local(
    state: Arc<AppState>,
    req: PortConnectionReadRequest,
) -> Result<PortConnectionReadResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let stream = tcp_connection(&state, &req.connection_id).await?;
    let mut reader = stream.reader.lock().await;
    let mut buf = vec![0u8; READ_BUF_SIZE];
    let shutdown = state.shutdown.clone();
    let read = tokio::select! {
        _ = shutdown.cancelled() => {
            return Err(port_connection_closed(&req.connection_id));
        }
        _ = stream.cancel.cancelled() => {
            return Err(port_connection_closed(&req.connection_id));
        }
        read = reader.read(&mut buf) => read.map_err(|err| {
            if stream.cancel.is_cancelled() {
                port_connection_closed(&req.connection_id)
            } else {
                rpc_error("port_read_failed", err.to_string())
            }
        })?,
    };
    if stream.cancel.is_cancelled() || state.shutdown.is_cancelled() {
        return Err(port_connection_closed(&req.connection_id));
    }
    if read == 0 {
        drop(reader);
        state
            .port_forwards
            .tcp_connections
            .lock()
            .await
            .remove(&req.connection_id);
        if let Some(lease_id) = &stream.lease_id {
            untrack_connection_lease(&state.port_forwards, lease_id, &req.connection_id).await;
        }
        return Ok(PortConnectionReadResponse {
            data: String::new(),
            eof: true,
        });
    }
    buf.truncate(read);
    Ok(PortConnectionReadResponse {
        data: encode_bytes(&buf),
        eof: false,
    })
}

pub async fn connection_write_local(
    state: Arc<AppState>,
    req: PortConnectionWriteRequest,
) -> Result<EmptyResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let bytes = decode_bytes(&req.data)?;
    let stream = tcp_connection(&state, &req.connection_id).await?;
    if stream.cancel.is_cancelled() || state.shutdown.is_cancelled() {
        return Err(port_connection_closed(&req.connection_id));
    }
    let mut writer = stream.writer.lock().await;
    let mut written = 0usize;
    let shutdown = state.shutdown.clone();
    while written < bytes.len() {
        if stream.cancel.is_cancelled() || state.shutdown.is_cancelled() {
            return Err(port_connection_closed(&req.connection_id));
        }
        let count = tokio::select! {
            _ = shutdown.cancelled() => {
                return Err(port_connection_closed(&req.connection_id));
            }
            _ = stream.cancel.cancelled() => {
                return Err(port_connection_closed(&req.connection_id));
            }
            write = writer.write(&bytes[written..]) => write.map_err(|err| {
                if stream.cancel.is_cancelled() {
                    port_connection_closed(&req.connection_id)
                } else {
                    rpc_error("port_write_failed", err.to_string())
                }
            })?,
        };
        if count == 0 {
            return Err(rpc_error(
                "port_write_failed",
                format!(
                    "connection `{}` returned zero-byte write",
                    req.connection_id
                ),
            ));
        }
        written += count;
    }
    Ok(EmptyResponse {})
}

pub async fn connection_close_local(
    state: Arc<AppState>,
    req: PortConnectionCloseRequest,
) -> Result<EmptyResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let connection = state
        .port_forwards
        .tcp_connections
        .lock()
        .await
        .remove(&req.connection_id);
    if let Some(connection) = connection {
        if let Some(lease_id) = &connection.lease_id {
            untrack_connection_lease(&state.port_forwards, lease_id, &req.connection_id).await;
        }
        connection.cancel.cancel();
    }
    tracing::debug!(
        target = %state.config.target,
        connection_id = %req.connection_id,
        "closed port forward tcp connection"
    );
    Ok(EmptyResponse {})
}

pub async fn udp_datagram_read_local(
    state: Arc<AppState>,
    req: PortUdpDatagramReadRequest,
) -> Result<PortUdpDatagramReadResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let socket = udp_socket(&state, &req.bind_id).await?;
    let mut buf = vec![0u8; READ_BUF_SIZE];
    let shutdown = state.shutdown.clone();
    let (read, peer) = tokio::select! {
        _ = shutdown.cancelled() => {
            return Err(port_bind_closed(&req.bind_id));
        }
        _ = socket.cancel.cancelled() => {
            return Err(port_bind_closed(&req.bind_id));
        }
        read = socket.socket.recv_from(&mut buf) => read.map_err(|err| {
            if socket.cancel.is_cancelled() {
                port_bind_closed(&req.bind_id)
            } else {
                rpc_error("port_read_failed", err.to_string())
            }
        })?,
    };
    if socket.cancel.is_cancelled() || state.shutdown.is_cancelled() {
        return Err(port_bind_closed(&req.bind_id));
    }
    buf.truncate(read);
    Ok(PortUdpDatagramReadResponse {
        peer: peer.to_string(),
        data: encode_bytes(&buf),
    })
}

pub async fn udp_datagram_write_local(
    state: Arc<AppState>,
    req: PortUdpDatagramWriteRequest,
) -> Result<EmptyResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    let bytes = decode_bytes(&req.data)?;
    let peer = resolve_endpoint(&req.peer)
        .await
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let socket = udp_socket(&state, &req.bind_id).await?;
    if socket.cancel.is_cancelled() || state.shutdown.is_cancelled() {
        return Err(port_bind_closed(&req.bind_id));
    }
    let shutdown = state.shutdown.clone();
    tokio::select! {
        _ = shutdown.cancelled() => {
            return Err(port_bind_closed(&req.bind_id));
        }
        _ = socket.cancel.cancelled() => {
            return Err(port_bind_closed(&req.bind_id));
        }
        write = socket.socket.send_to(&bytes, peer) => {
            write.map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
        }
    }
    Ok(EmptyResponse {})
}

pub async fn lease_renew_local(
    state: Arc<AppState>,
    req: PortLeaseRenewRequest,
) -> Result<EmptyResponse, HostRpcError> {
    sweep_expired_port_forwards(&state).await;
    renew_port_forward_lease(&state.port_forwards, &req.lease_id, req.ttl_ms).await?;
    Ok(EmptyResponse {})
}

async fn listen_tcp(
    state: Arc<AppState>,
    endpoint: &str,
    lease: Option<PortForwardLease>,
) -> Result<PortListenResponse, HostRpcError> {
    let listener = TcpListener::bind(endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bind_id = format!("bind_{}", uuid::Uuid::new_v4().simple());
    let lease_id = lease.as_ref().map(|lease| lease.lease_id.clone());
    state.port_forwards.tcp_listeners.lock().await.insert(
        bind_id.clone(),
        Arc::new(TcpListenerEntry {
            listener,
            cancel: CancellationToken::new(),
            lease_id: lease_id.clone(),
        }),
    );
    if let Some(lease) = lease {
        register_bind_lease(&state.port_forwards, lease, &bind_id).await?;
    }
    tracing::info!(
        target = %state.config.target,
        bind_id = %bind_id,
        endpoint = %bound_endpoint,
        protocol = "tcp",
        "opened port forward listener"
    );
    Ok(PortListenResponse {
        bind_id,
        endpoint: bound_endpoint.to_string(),
    })
}

async fn listen_udp(
    state: Arc<AppState>,
    endpoint: &str,
    lease: Option<PortForwardLease>,
) -> Result<PortListenResponse, HostRpcError> {
    let socket = UdpSocket::bind(endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = socket
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bind_id = format!("bind_{}", uuid::Uuid::new_v4().simple());
    let lease_id = lease.as_ref().map(|lease| lease.lease_id.clone());
    state.port_forwards.udp_sockets.lock().await.insert(
        bind_id.clone(),
        Arc::new(UdpSocketEntry {
            socket,
            cancel: CancellationToken::new(),
            lease_id: lease_id.clone(),
        }),
    );
    if let Some(lease) = lease {
        register_bind_lease(&state.port_forwards, lease, &bind_id).await?;
    }
    tracing::info!(
        target = %state.config.target,
        bind_id = %bind_id,
        endpoint = %bound_endpoint,
        protocol = "udp",
        "opened port forward listener"
    );
    Ok(PortListenResponse {
        bind_id,
        endpoint: bound_endpoint.to_string(),
    })
}

async fn connect_tcp(
    state: Arc<AppState>,
    endpoint: &str,
    lease: Option<PortForwardLease>,
) -> Result<PortConnectResponse, HostRpcError> {
    let endpoint = ensure_nonzero_connect_endpoint(endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let shutdown = state.shutdown.clone();
    let stream = tokio::select! {
        _ = shutdown.cancelled() => {
            return Err(rpc_error("port_connect_failed", "daemon is shutting down"));
        }
        connect = TcpStream::connect(endpoint.as_str()) => connect
            .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?,
    };
    let connection_id = format!("conn_{}", uuid::Uuid::new_v4().simple());
    let lease_id = lease.as_ref().map(|lease| lease.lease_id.clone());
    state.port_forwards.tcp_connections.lock().await.insert(
        connection_id.clone(),
        Arc::new(TcpConnection::new(stream, lease_id.clone())),
    );
    if let Some(lease) = lease {
        register_connection_lease(&state.port_forwards, lease, &connection_id).await?;
    }
    tracing::debug!(
        target = %state.config.target,
        connection_id = %connection_id,
        endpoint = %endpoint,
        "opened port forward tcp connection"
    );
    Ok(PortConnectResponse { connection_id })
}

async fn tcp_connection(
    state: &AppState,
    connection_id: &str,
) -> Result<Arc<TcpConnection>, HostRpcError> {
    state
        .port_forwards
        .tcp_connections
        .lock()
        .await
        .get(connection_id)
        .cloned()
        .ok_or_else(|| {
            rpc_error(
                "unknown_port_connection",
                format!("unknown connection `{connection_id}`"),
            )
        })
}

async fn tcp_listener(
    state: &AppState,
    bind_id: &str,
) -> Result<Arc<TcpListenerEntry>, HostRpcError> {
    state
        .port_forwards
        .tcp_listeners
        .lock()
        .await
        .get(bind_id)
        .cloned()
        .ok_or_else(|| rpc_error("unknown_port_bind", format!("unknown bind `{bind_id}`")))
}

async fn udp_socket(state: &AppState, bind_id: &str) -> Result<Arc<UdpSocketEntry>, HostRpcError> {
    state
        .port_forwards
        .udp_sockets
        .lock()
        .await
        .get(bind_id)
        .cloned()
        .ok_or_else(|| rpc_error("unknown_port_bind", format!("unknown bind `{bind_id}`")))
}

async fn register_bind_lease(
    state: &PortForwardState,
    lease: PortForwardLease,
    bind_id: &str,
) -> Result<(), HostRpcError> {
    let ttl = lease_ttl(lease.ttl_ms)?;
    let mut leases = state.leases.lock().await;
    let entry = leases.entry(lease.lease_id).or_insert_with(|| LeaseEntry {
        expires_at: Instant::now() + ttl,
        binds: HashSet::new(),
        connections: HashSet::new(),
    });
    entry.expires_at = Instant::now() + ttl;
    entry.binds.insert(bind_id.to_string());
    Ok(())
}

async fn register_connection_lease(
    state: &PortForwardState,
    lease: PortForwardLease,
    connection_id: &str,
) -> Result<(), HostRpcError> {
    let ttl = lease_ttl(lease.ttl_ms)?;
    let mut leases = state.leases.lock().await;
    let entry = leases.entry(lease.lease_id).or_insert_with(|| LeaseEntry {
        expires_at: Instant::now() + ttl,
        binds: HashSet::new(),
        connections: HashSet::new(),
    });
    entry.expires_at = Instant::now() + ttl;
    entry.connections.insert(connection_id.to_string());
    Ok(())
}

async fn renew_port_forward_lease(
    state: &PortForwardState,
    lease_id: &str,
    ttl_ms: u64,
) -> Result<(), HostRpcError> {
    let ttl = lease_ttl(ttl_ms)?;
    let mut leases = state.leases.lock().await;
    if let Some(entry) = leases.get_mut(lease_id) {
        // Late renewals can race with expiry cleanup after a broker crash; treat them as a no-op.
        entry.expires_at = Instant::now() + ttl;
    }
    Ok(())
}

async fn track_connection_lease(state: &PortForwardState, lease_id: &str, connection_id: &str) {
    let mut leases = state.leases.lock().await;
    if let Some(entry) = leases.get_mut(lease_id) {
        entry.connections.insert(connection_id.to_string());
    }
}

async fn untrack_bind_lease(state: &PortForwardState, lease_id: &str, bind_id: &str) {
    let mut leases = state.leases.lock().await;
    remove_bind_from_lease(&mut leases, lease_id, bind_id);
}

async fn untrack_connection_lease(state: &PortForwardState, lease_id: &str, connection_id: &str) {
    let mut leases = state.leases.lock().await;
    remove_connection_from_lease(&mut leases, lease_id, connection_id);
}

async fn sweep_expired_port_forwards(state: &AppState) {
    let expired = {
        let mut leases = state.port_forwards.leases.lock().await;
        let now = Instant::now();
        let expired_ids = leases
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(lease_id, _)| lease_id.clone())
            .collect::<Vec<_>>();
        let mut expired = Vec::new();
        for lease_id in expired_ids {
            if let Some(entry) = leases.remove(&lease_id) {
                expired.push(entry);
            }
        }
        expired
    };

    for entry in expired {
        expire_binds(&state.port_forwards, entry.binds).await;
        expire_connections(&state.port_forwards, entry.connections).await;
    }
}

async fn expire_binds(state: &PortForwardState, bind_ids: HashSet<String>) {
    for bind_id in bind_ids {
        if let Some(listener) = state.tcp_listeners.lock().await.remove(&bind_id) {
            listener.cancel.cancel();
        }
        if let Some(socket) = state.udp_sockets.lock().await.remove(&bind_id) {
            socket.cancel.cancel();
        }
    }
}

async fn expire_connections(state: &PortForwardState, connection_ids: HashSet<String>) {
    for connection_id in connection_ids {
        if let Some(connection) = state.tcp_connections.lock().await.remove(&connection_id) {
            connection.cancel.cancel();
        }
    }
}

fn remove_bind_from_lease(leases: &mut HashMap<String, LeaseEntry>, lease_id: &str, bind_id: &str) {
    if let Some(entry) = leases.get_mut(lease_id) {
        entry.binds.remove(bind_id);
        if entry.binds.is_empty() && entry.connections.is_empty() {
            leases.remove(lease_id);
        }
    }
}

fn remove_connection_from_lease(
    leases: &mut HashMap<String, LeaseEntry>,
    lease_id: &str,
    connection_id: &str,
) {
    if let Some(entry) = leases.get_mut(lease_id) {
        entry.connections.remove(connection_id);
        if entry.binds.is_empty() && entry.connections.is_empty() {
            leases.remove(lease_id);
        }
    }
}

fn lease_ttl(ttl_ms: u64) -> Result<Duration, HostRpcError> {
    if ttl_ms == 0 {
        return Err(rpc_error(
            "invalid_port_lease",
            "port forward lease ttl_ms must be > 0",
        ));
    }
    Ok(Duration::from_millis(
        ttl_ms.max(EXPIRED_FORWARD_SWEEP_INTERVAL.as_millis() as u64),
    ))
}

async fn resolve_endpoint(endpoint: &str) -> anyhow::Result<SocketAddr> {
    let endpoint = ensure_nonzero_connect_endpoint(endpoint)?;
    resolve_socket_addr(endpoint).await
}

async fn resolve_socket_addr(endpoint: String) -> anyhow::Result<SocketAddr> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<SocketAddr> {
        endpoint
            .to_socket_addrs()
            .with_context(|| format!("resolving endpoint `{endpoint}`"))?
            .next()
            .ok_or_else(|| anyhow::anyhow!("endpoint `{endpoint}` did not resolve"))
    })
    .await
    .map_err(anyhow::Error::from)?
}

fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_bytes(data: &str) -> Result<Vec<u8>, HostRpcError> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| rpc_error("invalid_port_data", err.to_string()))
}

fn port_bind_closed(bind_id: &str) -> HostRpcError {
    rpc_error("port_bind_closed", format!("bind `{bind_id}` was closed"))
}

fn port_connection_closed(connection_id: &str) -> HostRpcError {
    rpc_error(
        "port_connection_closed",
        format!("connection `{connection_id}` was closed"),
    )
}

fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    HostRpcError {
        status: 400,
        code,
        message,
    }
}

#[cfg(test)]
mod port_tunnel_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, read_frame, write_frame, write_preface,
    };
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::{
        HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig, build_runtime_state,
    };

    #[tokio::test]
    async fn tunnel_binds_tcp_listener_and_releases_it_on_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state.clone(), daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": listen_endpoint }),
            ),
        )
        .await
        .unwrap();

        let ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ok.frame_type, FrameType::TcpListenOk);
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);

        wait_until_bindable(&bound_endpoint).await;
    }

    #[tokio::test]
    async fn tunnel_tcp_connect_echoes_binary_data_full_duplex() {
        let state = test_state();
        let echo_endpoint = spawn_tcp_echo_server().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": echo_endpoint }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        write_frame(
            &mut broker_side,
            &data_frame(FrameType::TcpData, 1, b"\0hello\xff".to_vec()),
        )
        .await
        .unwrap();
        let echoed = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(echoed.frame_type, FrameType::TcpData);
        assert_eq!(echoed.data, b"\0hello\xff");
    }

    #[tokio::test]
    async fn tunnel_udp_bind_relays_datagrams_from_two_peers() {
        let state = test_state();
        let endpoint = free_loopback_endpoint().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let bind_ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(bind_ok.frame_type, FrameType::UdpBindOk);
        let bound_endpoint = endpoint_from_frame(&bind_ok);

        let peer_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer_a.send_to(b"from-a", &bound_endpoint).await.unwrap();
        peer_b.send_to(b"from-b", &bound_endpoint).await.unwrap();

        let first = read_frame(&mut broker_side).await.unwrap();
        let second = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(first.frame_type, FrameType::UdpDatagram);
        assert_eq!(second.frame_type, FrameType::UdpDatagram);
        assert_eq!(
            sorted_payloads([first.data, second.data]),
            vec![b"from-a".to_vec(), b"from-b".to_vec()]
        );
    }

    #[tokio::test]
    async fn tunnel_reports_tcp_listen_errors_on_request_stream() {
        let state = test_state();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_endpoint = occupied.local_addr().unwrap().to_string();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                7,
                serde_json::json!({ "endpoint": occupied_endpoint }),
            ),
        )
        .await
        .unwrap();

        let error = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut broker_side))
            .await
            .expect("listen error frame should arrive")
            .unwrap();
        assert_eq!(error.frame_type, FrameType::Error);
        assert_eq!(error.stream_id, 7);
    }

    #[tokio::test]
    async fn tunnel_exits_promptly_when_host_shuts_down() {
        let state = test_state();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        let tunnel_task = tokio::spawn(serve_tunnel(state.clone(), daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        shutdown_local(&state).await;

        let result = tokio::time::timeout(Duration::from_secs(1), tunnel_task)
            .await
            .expect("tunnel should exit after host shutdown")
            .unwrap();
        result.unwrap();
    }

    fn test_state() -> Arc<AppState> {
        let workdir = std::env::temp_dir().join(format!(
            "remote-exec-host-port-tunnel-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workdir).unwrap();
        Arc::new(
            build_runtime_state(HostRuntimeConfig {
                target: "test".to_string(),
                default_workdir: workdir,
                windows_posix_root: None,
                sandbox: None,
                enable_transfer_compression: true,
                allow_login_shell: true,
                pty: PtyMode::None,
                default_shell: None,
                yield_time: YieldTimeConfig::default(),
                experimental_apply_patch_target_encoding_autodetect: false,
                process_environment: ProcessEnvironment::capture_current(),
            })
            .unwrap(),
        )
    }

    async fn free_loopback_endpoint() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        drop(listener);
        endpoint
    }

    async fn wait_until_bindable(endpoint: &str) {
        for _ in 0..40 {
            if tokio::net::TcpListener::bind(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("endpoint `{endpoint}` did not become bindable");
    }

    async fn spawn_tcp_echo_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
            let read = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..read]).await.unwrap();
        });
        endpoint
    }

    fn json_frame(frame_type: FrameType, stream_id: u32, meta: Value) -> Frame {
        Frame {
            frame_type,
            flags: 0,
            stream_id,
            meta: serde_json::to_vec(&meta).unwrap(),
            data: Vec::new(),
        }
    }

    fn data_frame(frame_type: FrameType, stream_id: u32, data: Vec<u8>) -> Frame {
        Frame {
            frame_type,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data,
        }
    }

    fn endpoint_from_frame(frame: &Frame) -> String {
        serde_json::from_slice::<Value>(&frame.meta).unwrap()["endpoint"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn sorted_payloads<const N: usize>(payloads: [Vec<u8>; N]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::from(payloads);
        payloads.sort();
        payloads
    }
}
