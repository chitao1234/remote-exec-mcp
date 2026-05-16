use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::port_tunnel::TunnelForwardProtocol;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForwardPortsInput {
    Open {
        listen_side: String,
        connect_side: String,
        forwards: Vec<ForwardPortSpec>,
    },
    List {
        #[serde(default)]
        listen_side: Option<String>,
        #[serde(default)]
        connect_side: Option<String>,
        #[serde(default)]
        forward_ids: Vec<String>,
    },
    Close {
        forward_ids: Vec<String>,
    },
}

impl ForwardPortsInput {
    pub fn action(&self) -> ForwardPortsAction {
        match self {
            Self::Open { .. } => ForwardPortsAction::Open,
            Self::List { .. } => ForwardPortsAction::List,
            Self::Close { .. } => ForwardPortsAction::Close,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardPortSpec {
    pub listen_endpoint: String,
    pub connect_endpoint: String,
    pub protocol: ForwardPortProtocol,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortProtocol {
    #[default]
    Tcp,
    Udp,
}

impl From<ForwardPortProtocol> for TunnelForwardProtocol {
    fn from(value: ForwardPortProtocol) -> Self {
        match value {
            ForwardPortProtocol::Tcp => Self::Tcp,
            ForwardPortProtocol::Udp => Self::Udp,
        }
    }
}

impl From<TunnelForwardProtocol> for ForwardPortProtocol {
    fn from(value: TunnelForwardProtocol) -> Self {
        match value {
            TunnelForwardProtocol::Tcp => Self::Tcp,
            TunnelForwardProtocol::Udp => Self::Udp,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ForwardPortsResult {
    pub action: ForwardPortsAction,
    pub forwards: Vec<ForwardPortEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortsAction {
    Open,
    List,
    Close,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ForwardPortEntry {
    pub forward_id: String,
    pub listen_side: String,
    pub listen_endpoint: String,
    pub connect_side: String,
    pub connect_endpoint: String,
    pub protocol: ForwardPortProtocol,
    pub status: ForwardPortStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub phase: ForwardPortPhase,
    pub listen_state: ForwardPortSideState,
    pub connect_state: ForwardPortSideState,
    pub active_tcp_streams: u64,
    pub dropped_tcp_streams: u64,
    pub dropped_udp_datagrams: u64,
    pub reconnect_attempts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reconnect_at: Option<Timestamp>,
    pub limits: ForwardPortLimitSummary,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct Timestamp(pub String);

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortStatus {
    Open,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortPhase {
    Opening,
    Ready,
    Reconnecting,
    Draining,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortSideRole {
    Listen,
    Connect,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortSideHealth {
    Starting,
    Ready,
    Reconnecting,
    Degraded,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ForwardPortSideState {
    pub side: String,
    pub role: ForwardPortSideRole,
    pub generation: u64,
    pub health: ForwardPortSideHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ForwardPortLimitSummary {
    pub max_active_tcp_streams: u64,
    pub max_udp_peers: u64,
    pub max_pending_tcp_bytes_per_stream: u64,
    pub max_pending_tcp_bytes_per_forward: u64,
    pub max_tunnel_queued_bytes: u64,
    pub max_reconnecting_forwards: usize,
}

impl ForwardPortEntry {
    pub fn new_open(
        forward_id: String,
        listen_side: String,
        listen_endpoint: String,
        connect_side: String,
        connect_endpoint: String,
        protocol: ForwardPortProtocol,
        limits: ForwardPortLimitSummary,
    ) -> Self {
        Self {
            forward_id,
            listen_side: listen_side.clone(),
            listen_endpoint,
            connect_side: connect_side.clone(),
            connect_endpoint,
            protocol,
            status: ForwardPortStatus::Open,
            last_error: None,
            phase: ForwardPortPhase::Ready,
            listen_state: ForwardPortSideState {
                side: listen_side,
                role: ForwardPortSideRole::Listen,
                generation: 1,
                health: ForwardPortSideHealth::Ready,
                last_error: None,
            },
            connect_state: ForwardPortSideState {
                side: connect_side,
                role: ForwardPortSideRole::Connect,
                generation: 1,
                health: ForwardPortSideHealth::Ready,
                last_error: None,
            },
            active_tcp_streams: 0,
            dropped_tcp_streams: 0,
            dropped_udp_datagrams: 0,
            reconnect_attempts: 0,
            last_reconnect_at: None,
            limits,
        }
    }
}
