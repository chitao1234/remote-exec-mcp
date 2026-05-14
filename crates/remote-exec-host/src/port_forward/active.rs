use std::sync::Arc;
use std::sync::atomic::Ordering;

use remote_exec_proto::port_tunnel::TunnelForwardProtocol;
use remote_exec_proto::rpc::RpcErrorCode;

use crate::HostRpcError;

use super::error::{operational_error, request_error};
use super::session::{AttachmentState, SessionState};
use super::{
    ActiveTunnelState, ConnectRuntimeState, TunnelSender, TunnelState, tunnel_error_frame,
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

#[derive(Clone, Copy)]
enum RequiredTunnelAccess {
    AnyProtocol,
    ListenOnly,
    ConnectOnly,
    BindTarget,
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

impl ActiveProtocolAccess {
    pub(super) fn tcp_streams(
        &self,
    ) -> &tokio::sync::Mutex<std::collections::HashMap<u32, super::TcpStreamEntry>> {
        match self {
            Self::Listen(listen) => listen.tcp_streams(),
            Self::Connect(connect) => connect.tcp_streams(),
        }
    }
}

impl ActiveTunnelAccess {
    pub(super) fn require_protocol(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ActiveProtocolAccess, HostRpcError> {
        self.require_access(protocol, operation, RequiredTunnelAccess::AnyProtocol)
    }

    pub(super) fn require_listen_session(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ListenContext, HostRpcError> {
        match self.require_access(protocol, operation, RequiredTunnelAccess::ListenOnly)? {
            ActiveProtocolAccess::Listen(context) => Ok(context),
            ActiveProtocolAccess::Connect(_) => unreachable!("listen-only access accepted connect"),
        }
    }

    pub(super) fn require_connect_tunnel(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ConnectContext, HostRpcError> {
        match self.require_access(protocol, operation, RequiredTunnelAccess::ConnectOnly)? {
            ActiveProtocolAccess::Connect(context) => Ok(context),
            ActiveProtocolAccess::Listen(_) => {
                unreachable!("connect-only access accepted listen")
            }
        }
    }

    pub(super) fn require_bind_target(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<ActiveProtocolAccess, HostRpcError> {
        self.require_access(protocol, operation, RequiredTunnelAccess::BindTarget)
    }

    pub(super) fn protocol_access_if(
        self,
        protocol: TunnelForwardProtocol,
    ) -> Option<ActiveProtocolAccess> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Some(ActiveProtocolAccess::Listen(context)),
            Self::Connect {
                protocol: open_protocol,
                context,
            } if open_protocol == protocol => Some(ActiveProtocolAccess::Connect(context)),
            _ => None,
        }
    }

    pub(super) fn role_access(self) -> ActiveTunnelRole {
        match self {
            Self::Listen { context, .. } => ActiveTunnelRole::Listen(context),
            Self::Connect { context, .. } => ActiveTunnelRole::Connect(context),
            Self::Unopened => ActiveTunnelRole::Unopened,
        }
    }

    fn require_access(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
        required: RequiredTunnelAccess,
    ) -> Result<ActiveProtocolAccess, HostRpcError> {
        match self {
            Self::Unopened => Err(tunnel_open_required_error(operation)),
            Self::Listen {
                protocol: open_protocol,
                context,
            } => {
                if let Some(err) = required.role_mismatch_error(operation, "listen") {
                    return Err(err);
                }
                if open_protocol == protocol {
                    Ok(ActiveProtocolAccess::Listen(context))
                } else {
                    Err(required.protocol_mismatch_error(operation, protocol, "listen"))
                }
            }
            Self::Connect {
                protocol: open_protocol,
                context,
            } => {
                if let Some(err) = required.role_mismatch_error(operation, "connect") {
                    return Err(err);
                }
                if open_protocol == protocol {
                    Ok(ActiveProtocolAccess::Connect(context))
                } else {
                    Err(required.protocol_mismatch_error(operation, protocol, "connect"))
                }
            }
        }
    }
}

impl RequiredTunnelAccess {
    fn role_mismatch_error(
        self,
        operation: &str,
        actual_role: &'static str,
    ) -> Option<HostRpcError> {
        match (self, actual_role) {
            (Self::ListenOnly, "connect") => Some(role_required_error(operation, "listen")),
            (Self::ConnectOnly, "listen") => Some(role_required_error(operation, "connect")),
            _ => None,
        }
    }

    fn protocol_mismatch_error(
        self,
        operation: &str,
        protocol: TunnelForwardProtocol,
        actual_role: &'static str,
    ) -> HostRpcError {
        match self {
            Self::AnyProtocol => protocol_required_error(operation, protocol, None),
            Self::ListenOnly | Self::BindTarget if actual_role == "listen" => {
                protocol_required_error(operation, protocol, Some("listen"))
            }
            Self::ConnectOnly | Self::BindTarget if actual_role == "connect" => {
                protocol_required_error(operation, protocol, Some("connect"))
            }
            _ => unreachable!("role mismatch should be handled before protocol mismatch"),
        }
    }
}

fn tunnel_open_required_error(operation: &str) -> HostRpcError {
    request_error(
        RpcErrorCode::InvalidPortTunnel,
        format!("{operation} requires tunnel open"),
    )
}

fn role_required_error(operation: &str, role: &str) -> HostRpcError {
    request_error(
        RpcErrorCode::InvalidPortTunnel,
        format!("{operation} requires an open {role} tunnel"),
    )
}

fn protocol_required_error(
    operation: &str,
    protocol: TunnelForwardProtocol,
    role: Option<&str>,
) -> HostRpcError {
    let protocol = protocol_label(protocol);
    let role = role.map(|value| format!(" {value}")).unwrap_or_default();
    request_error(
        RpcErrorCode::InvalidPortTunnel,
        format!("{operation} requires an open {protocol}{role} tunnel"),
    )
}

pub(super) async fn active_access(
    tunnel: &Arc<TunnelState>,
) -> Result<ActiveTunnelAccess, HostRpcError> {
    match tunnel.active.lock().await.clone() {
        ActiveTunnelState::Unopened => Ok(ActiveTunnelAccess::Unopened),
        ActiveTunnelState::Connect { protocol, runtime } => Ok(ActiveTunnelAccess::Connect {
            protocol,
            context: ConnectContext { runtime },
        }),
        ActiveTunnelState::Listen { protocol, session } => Ok(ActiveTunnelAccess::Listen {
            protocol,
            context: current_listen_context(session).await?,
        }),
    }
}

pub(super) async fn connection_generation(tunnel: &Arc<TunnelState>) -> Option<u64> {
    match tunnel.active.lock().await.clone() {
        ActiveTunnelState::Connect { runtime, .. } => Some(runtime.generation),
        ActiveTunnelState::Listen { session, .. } => {
            Some(session.generation.load(Ordering::Acquire))
        }
        ActiveTunnelState::Unopened => {
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

async fn current_listen_context(session: Arc<SessionState>) -> Result<ListenContext, HostRpcError> {
    let attachment = session.current_attachment().await.ok_or_else(|| {
        operational_error(
            RpcErrorCode::PortTunnelClosed,
            "port tunnel attachment is closed",
        )
    })?;
    Ok(ListenContext::new(session, attachment))
}
