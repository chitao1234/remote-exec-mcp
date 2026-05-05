use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use anyhow::Context;
use base64::Engine;
use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectRequest, PortConnectResponse, PortConnectionCloseRequest,
    PortConnectionReadRequest, PortConnectionReadResponse, PortConnectionWriteRequest,
    PortForwardProtocol, PortListenAcceptRequest, PortListenAcceptResponse, PortListenCloseRequest,
    PortListenRequest, PortListenResponse, PortUdpDatagramReadRequest, PortUdpDatagramReadResponse,
    PortUdpDatagramWriteRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{AppState, HostRpcError};

const READ_BUF_SIZE: usize = 64 * 1024;

#[derive(Clone, Default)]
pub struct PortForwardState {
    tcp_listeners: Arc<Mutex<HashMap<String, Arc<TcpListenerEntry>>>>,
    udp_sockets: Arc<Mutex<HashMap<String, Arc<UdpSocketEntry>>>>,
    tcp_connections: Arc<Mutex<HashMap<String, Arc<TcpConnection>>>>,
}

struct TcpListenerEntry {
    listener: TcpListener,
    cancel: CancellationToken,
}

struct UdpSocketEntry {
    socket: UdpSocket,
    cancel: CancellationToken,
}

struct TcpConnection {
    reader: Mutex<OwnedReadHalf>,
    writer: Mutex<OwnedWriteHalf>,
    cancel: CancellationToken,
}

impl TcpConnection {
    fn new(stream: TcpStream) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            cancel: CancellationToken::new(),
        }
    }
}

pub async fn listen_local(
    state: Arc<AppState>,
    req: PortListenRequest,
) -> Result<PortListenResponse, HostRpcError> {
    let endpoint = normalize_endpoint(&req.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    match req.protocol {
        PortForwardProtocol::Tcp => listen_tcp(state, &endpoint).await,
        PortForwardProtocol::Udp => listen_udp(state, &endpoint).await,
    }
}

pub async fn listen_accept_local(
    state: Arc<AppState>,
    req: PortListenAcceptRequest,
) -> Result<PortListenAcceptResponse, HostRpcError> {
    let listener = tcp_listener(&state, &req.bind_id).await?;
    let (stream, peer_addr) = tokio::select! {
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
    if listener.cancel.is_cancelled() {
        return Err(port_bind_closed(&req.bind_id));
    }
    let connection_id = format!("conn_{}", uuid::Uuid::new_v4().simple());
    state
        .port_forwards
        .tcp_connections
        .lock()
        .await
        .insert(connection_id.clone(), Arc::new(TcpConnection::new(stream)));
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
        listener.cancel.cancel();
    }
    if let Some(socket) = udp_socket {
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
    match req.protocol {
        PortForwardProtocol::Tcp => connect_tcp(state, &req.endpoint).await,
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
    let stream = tcp_connection(&state, &req.connection_id).await?;
    let mut reader = stream.reader.lock().await;
    let mut buf = vec![0u8; READ_BUF_SIZE];
    let read = tokio::select! {
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
    if stream.cancel.is_cancelled() {
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
    let bytes = decode_bytes(&req.data)?;
    let stream = tcp_connection(&state, &req.connection_id).await?;
    if stream.cancel.is_cancelled() {
        return Err(port_connection_closed(&req.connection_id));
    }
    let mut writer = stream.writer.lock().await;
    let mut written = 0usize;
    while written < bytes.len() {
        if stream.cancel.is_cancelled() {
            return Err(port_connection_closed(&req.connection_id));
        }
        let count = tokio::select! {
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
    let connection = state
        .port_forwards
        .tcp_connections
        .lock()
        .await
        .remove(&req.connection_id);
    if let Some(connection) = connection {
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
    let socket = udp_socket(&state, &req.bind_id).await?;
    let mut buf = vec![0u8; READ_BUF_SIZE];
    let (read, peer) = tokio::select! {
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
    if socket.cancel.is_cancelled() {
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
    let bytes = decode_bytes(&req.data)?;
    let peer = resolve_endpoint(&req.peer)
        .await
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let socket = udp_socket(&state, &req.bind_id).await?;
    if socket.cancel.is_cancelled() {
        return Err(port_bind_closed(&req.bind_id));
    }
    socket
        .socket
        .send_to(&bytes, peer)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    Ok(EmptyResponse {})
}

async fn listen_tcp(
    state: Arc<AppState>,
    endpoint: &str,
) -> Result<PortListenResponse, HostRpcError> {
    let listener = TcpListener::bind(endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bind_id = format!("bind_{}", uuid::Uuid::new_v4().simple());
    state.port_forwards.tcp_listeners.lock().await.insert(
        bind_id.clone(),
        Arc::new(TcpListenerEntry {
            listener,
            cancel: CancellationToken::new(),
        }),
    );
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
) -> Result<PortListenResponse, HostRpcError> {
    let socket = UdpSocket::bind(endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = socket
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bind_id = format!("bind_{}", uuid::Uuid::new_v4().simple());
    state.port_forwards.udp_sockets.lock().await.insert(
        bind_id.clone(),
        Arc::new(UdpSocketEntry {
            socket,
            cancel: CancellationToken::new(),
        }),
    );
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
) -> Result<PortConnectResponse, HostRpcError> {
    let endpoint = ensure_nonzero_connect_endpoint(endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let stream = TcpStream::connect(endpoint.as_str())
        .await
        .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?;
    let connection_id = format!("conn_{}", uuid::Uuid::new_v4().simple());
    state
        .port_forwards
        .tcp_connections
        .lock()
        .await
        .insert(connection_id.clone(), Arc::new(TcpConnection::new(stream)));
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
