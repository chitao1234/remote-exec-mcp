use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use remote_exec_proto::port_tunnel::{Frame, TunnelForwardProtocol};
use serde::Serialize;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::AppState;

use super::limiter::{PortForwardLimiter, PortForwardPermit};
use super::session;

pub(super) struct TunnelState {
    pub(super) state: Arc<AppState>,
    pub(super) cancel: CancellationToken,
    pub(super) tx: TunnelSender,
    pub(super) open_mode: Mutex<TunnelMode>,
    pub(super) last_generation: AtomicU64,
    pub(super) active: Mutex<Option<ActiveTunnelState>>,
    pub(super) _connection_permit: PortForwardPermit,
}

#[derive(Clone)]
pub(super) enum ActiveTunnelState {
    Connect(Arc<ConnectRuntimeState>),
    Listen(Arc<session::SessionState>),
}

#[derive(Clone)]
pub(super) struct TunnelSender {
    pub(super) tx: mpsc::Sender<QueuedFrame>,
    pub(super) limiter: Arc<PortForwardLimiter>,
}

pub(super) struct QueuedFrame {
    pub(super) frame: Frame,
    pub(super) _permit: Option<PortForwardPermit>,
}

#[derive(Clone)]
pub(super) struct TcpWriterHandle {
    pub(super) tx: mpsc::Sender<TcpWriteCommand>,
    pub(super) cancel: CancellationToken,
}

pub(super) struct TcpStreamEntry {
    pub(super) writer: TcpWriterHandle,
    pub(super) _permit: PortForwardPermit,
    pub(super) cancel: Option<CancellationToken>,
}

pub(super) enum TcpWriteCommand {
    Data(Vec<u8>),
    Shutdown,
}

pub(super) struct ConnectionLocalUdpBind {
    pub(super) socket: Arc<UdpSocket>,
    pub(super) _permit: PortForwardPermit,
    pub(super) cancel: CancellationToken,
}

pub(super) struct ConnectRuntimeState {
    pub(super) tx: TunnelSender,
    pub(super) cancel: CancellationToken,
    pub(super) generation: u64,
    pub(super) tcp_streams: Mutex<HashMap<u32, TcpStreamEntry>>,
    pub(super) udp_binds: Mutex<HashMap<u32, ConnectionLocalUdpBind>>,
}

pub(super) struct UdpReaderEntry {
    pub(super) cancel: CancellationToken,
}

#[derive(Debug, Serialize)]
pub(super) struct EndpointOkMeta {
    pub(super) endpoint: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ErrorMeta {
    pub(super) code: String,
    pub(super) message: String,
    pub(super) fatal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) generation: Option<u64>,
}

#[derive(Clone)]
pub(super) enum TunnelMode {
    Unopened,
    Connect { protocol: TunnelForwardProtocol },
    Listen { protocol: TunnelForwardProtocol },
}
