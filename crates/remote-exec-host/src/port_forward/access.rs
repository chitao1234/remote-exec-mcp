use std::sync::Arc;

use remote_exec_proto::port_tunnel::TunnelForwardProtocol;
use remote_exec_proto::rpc::RpcErrorCode;

use crate::HostRpcError;

use super::error::rpc_error;
use super::session::SessionState;
use super::tunnel::tunnel_mode;
use super::{TunnelMode, TunnelState};

pub(super) enum OpenProtocolAccess {
    Listen(Arc<SessionState>),
    Connect,
}

pub(super) enum OpenTunnelRole {
    Listen(Arc<SessionState>),
    Connect,
    Unopened,
}

pub(super) enum TunnelAccess {
    Unopened,
    Connect {
        protocol: TunnelForwardProtocol,
    },
    Listen {
        protocol: TunnelForwardProtocol,
        session: Arc<SessionState>,
    },
}

impl TunnelAccess {
    pub(super) fn protocol_access(
        self,
        protocol: TunnelForwardProtocol,
    ) -> Option<OpenProtocolAccess> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                session,
            } if open_protocol == protocol => Some(OpenProtocolAccess::Listen(session)),
            Self::Connect {
                protocol: open_protocol,
            } if open_protocol == protocol => Some(OpenProtocolAccess::Connect),
            Self::Unopened | Self::Connect { .. } | Self::Listen { .. } => None,
        }
    }

    pub(super) fn require_protocol(
        self,
        protocol: TunnelForwardProtocol,
        operation: &str,
    ) -> Result<OpenProtocolAccess, HostRpcError> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                session,
            } if open_protocol == protocol => Ok(OpenProtocolAccess::Listen(session)),
            Self::Connect {
                protocol: open_protocol,
            } if open_protocol == protocol => Ok(OpenProtocolAccess::Connect),
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
    ) -> Result<Arc<SessionState>, HostRpcError> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                session,
            } if open_protocol == protocol => Ok(session),
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
    ) -> Result<(), HostRpcError> {
        match self {
            Self::Connect {
                protocol: open_protocol,
            } if open_protocol == protocol => Ok(()),
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
    ) -> Result<OpenProtocolAccess, HostRpcError> {
        match self {
            Self::Listen {
                protocol: open_protocol,
                session,
            } if open_protocol == protocol => Ok(OpenProtocolAccess::Listen(session)),
            Self::Listen { .. } => Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                format!(
                    "{operation} requires an open {} listen tunnel",
                    protocol_label(protocol)
                ),
            )),
            Self::Connect {
                protocol: open_protocol,
            } if open_protocol == protocol => Ok(OpenProtocolAccess::Connect),
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

    pub(super) fn role_access(self) -> OpenTunnelRole {
        match self {
            Self::Listen { session, .. } => OpenTunnelRole::Listen(session),
            Self::Connect { .. } => OpenTunnelRole::Connect,
            Self::Unopened => OpenTunnelRole::Unopened,
        }
    }
}

pub(super) async fn tunnel_access(tunnel: &Arc<TunnelState>) -> TunnelAccess {
    match tunnel_mode(tunnel).await {
        TunnelMode::Unopened => TunnelAccess::Unopened,
        TunnelMode::Connect { protocol } => TunnelAccess::Connect { protocol },
        TunnelMode::Listen { protocol, session } => TunnelAccess::Listen { protocol, session },
    }
}

fn protocol_label(protocol: TunnelForwardProtocol) -> &'static str {
    match protocol {
        TunnelForwardProtocol::Tcp => "tcp",
        TunnelForwardProtocol::Udp => "udp",
    }
}
