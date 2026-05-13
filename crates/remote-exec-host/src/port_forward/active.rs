use std::sync::Arc;
use std::sync::atomic::Ordering;

use remote_exec_proto::port_tunnel::TunnelForwardProtocol;
use remote_exec_proto::rpc::RpcErrorCode;

use crate::HostRpcError;

use super::error::rpc_error;
use super::session::{AttachmentState, SessionState};
use super::tunnel::tunnel_mode;
use super::{
    ActiveTunnelState, ConnectRuntimeState, TunnelMode, TunnelSender, TunnelState,
    tunnel_error_frame,
};

#[derive(Clone)]
pub(super) struct ConnectContext {
    runtime: Arc<ConnectRuntimeState>,
}

#[derive(Clone)]
pub(super) struct ListenContext {
    session: Arc<SessionState>,
    attachment: Arc<AttachmentState>,
}

pub(super) enum ActiveProtocolAccess {
    Listen(ListenContext),
    Connect(ConnectContext),
}

pub(super) enum ActiveTunnelRole {
    Listen(ListenContext),
    Connect(ConnectContext),
    Unopened,
}

pub(super) enum ActiveTunnelAccess {
    Unopened,
    Connect {
        protocol: TunnelForwardProtocol,
        context: ConnectContext,
    },
    Listen {
        protocol: TunnelForwardProtocol,
        context: ListenContext,
    },
}

impl ConnectContext {
    pub(super) fn tx(&self) -> &TunnelSender {
        &self.runtime.tx
    }

    pub(super) fn cancel(&self) -> &tokio_util::sync::CancellationToken {
        &self.runtime.cancel
    }

    pub(super) fn generation(&self) -> u64 {
        self.runtime.generation
    }

    pub(super) fn tcp_streams(
        &self,
    ) -> &tokio::sync::Mutex<std::collections::HashMap<u32, super::TcpStreamEntry>> {
        &self.runtime.tcp_streams
    }

    pub(super) fn udp_binds(
        &self,
    ) -> &tokio::sync::Mutex<std::collections::HashMap<u32, super::ConnectionLocalUdpBind>> {
        &self.runtime.udp_binds
    }
}

impl ListenContext {
    pub(super) fn new(session: Arc<SessionState>, attachment: Arc<AttachmentState>) -> Self {
        Self {
            session,
            attachment,
        }
    }

    pub(super) fn tx(&self) -> &TunnelSender {
        &self.attachment.tx
    }

    pub(super) fn generation(&self) -> u64 {
        self.session.generation.load(Ordering::Acquire)
    }

    pub(super) fn session(&self) -> &Arc<SessionState> {
        &self.session
    }

    pub(super) fn tcp_streams(
        &self,
    ) -> &tokio::sync::Mutex<std::collections::HashMap<u32, super::TcpStreamEntry>> {
        &self.attachment.tcp_streams
    }

    pub(super) fn udp_readers(
        &self,
    ) -> &tokio::sync::Mutex<std::collections::HashMap<u32, super::UdpReaderEntry>> {
        &self.attachment.udp_readers
    }
}

impl ActiveTunnelAccess {
    pub(super) fn require_protocol(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ActiveProtocolAccess, HostRpcError> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Ok(ActiveProtocolAccess::Listen(context)),
            Self::Connect {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Ok(ActiveProtocolAccess::Connect(context)),
            Self::Unopened => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!("{operation} requires tunnel open"),
            )),
            Self::Connect { .. } | Self::Listen { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!(
                    "{operation} requires an open {} tunnel",
                    protocol_label(protocol)
                ),
            )),
        }
    }

    pub(super) fn require_listen_session(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ListenContext, HostRpcError> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Ok(context),
            Self::Listen { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!(
                    "{operation} requires an open {} listen tunnel",
                    protocol_label(protocol)
                ),
            )),
            Self::Connect { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!("{operation} requires an open listen tunnel"),
            )),
            Self::Unopened => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!("{operation} requires tunnel open"),
            )),
        }
    }

    pub(super) fn require_connect_tunnel(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ConnectContext, HostRpcError> {
        match self {
            Self::Connect {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Ok(context),
            Self::Connect { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!(
                    "{operation} requires an open {} connect tunnel",
                    protocol_label(protocol)
                ),
            )),
            Self::Listen { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!("{operation} requires an open connect tunnel"),
            )),
            Self::Unopened => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!("{operation} requires tunnel open"),
            )),
        }
    }

    pub(super) fn require_bind_target(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ActiveProtocolAccess, HostRpcError> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Ok(ActiveProtocolAccess::Listen(context)),
            Self::Listen { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!(
                    "{operation} requires an open {} listen tunnel",
                    protocol_label(protocol)
                ),
            )),
            Self::Connect {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Ok(ActiveProtocolAccess::Connect(context)),
            Self::Connect { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!(
                    "{operation} requires an open {} connect tunnel",
                    protocol_label(protocol)
                ),
            )),
            Self::Unopened => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!("{operation} requires tunnel open"),
            )),
        }
    }

    pub(super) fn role_access(self) -> ActiveTunnelRole {
        match self {
            Self::Listen { context, .. } => ActiveTunnelRole::Listen(context),
            Self::Connect { context, .. } => ActiveTunnelRole::Connect(context),
            Self::Unopened => ActiveTunnelRole::Unopened,
        }
    }
}

pub(super) async fn active_access(
    tunnel: &Arc<TunnelState>,
) -> Result<ActiveTunnelAccess, HostRpcError> {
    match tunnel_mode(tunnel).await {
        TunnelMode::Unopened => Ok(ActiveTunnelAccess::Unopened),
        TunnelMode::Connect { protocol } => Ok(ActiveTunnelAccess::Connect {
            protocol,
            context: current_connect_context(tunnel).await?,
        }),
        TunnelMode::Listen { protocol } => Ok(ActiveTunnelAccess::Listen {
            protocol,
            context: current_listen_context(tunnel).await?,
        }),
    }
}

pub(super) async fn connection_generation(tunnel: &Arc<TunnelState>) -> Option<u64> {
    match tunnel.active.lock().await.clone() {
        Some(ActiveTunnelState::Connect(runtime)) => Some(runtime.generation),
        Some(ActiveTunnelState::Listen(session)) => {
            Some(session.generation.load(Ordering::Acquire))
        }
        None => {
            let generation = tunnel.last_generation.load(Ordering::Acquire);
            (generation != 0).then_some(generation)
        }
    }
}

pub(super) async fn send_tunnel_error(
    tx: &TunnelSender,
    generation: Option<u64>,
    stream_id: u32,
    code: RpcErrorCode,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    send_tunnel_error_code(tx, generation, stream_id, code.wire_value(), message, fatal).await
}

pub(super) async fn send_tunnel_error_code(
    tx: &TunnelSender,
    generation: Option<u64>,
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    tx.send(tunnel_error_frame(
        stream_id, code, message, fatal, generation,
    )?)
    .await
}

fn protocol_label(protocol: TunnelForwardProtocol) -> &'static str {
    match protocol {
        TunnelForwardProtocol::Tcp => "tcp",
        TunnelForwardProtocol::Udp => "udp",
    }
}

async fn current_connect_context(
    tunnel: &Arc<TunnelState>,
) -> Result<ConnectContext, HostRpcError> {
    match tunnel.active.lock().await.clone() {
        Some(ActiveTunnelState::Connect(runtime)) => Ok(ConnectContext { runtime }),
        Some(ActiveTunnelState::Listen(_)) | None => Err(rpc_error(
            RpcErrorCode::InvalidPortTunnel,
            "connect tunnel runtime is unavailable",
        )),
    }
}

async fn current_listen_context(tunnel: &Arc<TunnelState>) -> Result<ListenContext, HostRpcError> {
    let session = match tunnel.active.lock().await.clone() {
        Some(ActiveTunnelState::Listen(session)) => session,
        Some(ActiveTunnelState::Connect(_)) | None => {
            return Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                "listen tunnel session is unavailable",
            ));
        }
    };
    let attachment = session.current_attachment().await.ok_or_else(|| {
        rpc_error(
            RpcErrorCode::PortTunnelClosed,
            "port tunnel attachment is closed",
        )
    })?;
    Ok(ListenContext::new(session, attachment))
}
