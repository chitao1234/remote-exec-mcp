use serde::{Deserialize, Serialize};

pub const TUNNEL_CLOSE_REASON_OPERATOR_CLOSE: &str = "operator_close";
pub const TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED: &str = "listener_open_failed";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelRole {
    Listen,
    Connect,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelForwardProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelOpenMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub side: String,
    pub generation: u64,
    pub protocol: TunnelForwardProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelReadyMeta {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_timeout_ms: Option<u64>,
    pub limits: TunnelLimitSummary,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelLimitSummary {
    pub max_active_tcp_streams: u64,
    pub max_udp_peers: u64,
    pub max_queued_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelCloseMeta {
    pub forward_id: String,
    pub generation: u64,
    reason: String,
}

impl TunnelCloseMeta {
    pub fn operator_close(forward_id: impl Into<String>, generation: u64) -> Self {
        Self {
            forward_id: forward_id.into(),
            generation,
            reason: TUNNEL_CLOSE_REASON_OPERATOR_CLOSE.to_string(),
        }
    }

    pub fn from_raw_reason(
        forward_id: impl Into<String>,
        generation: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            forward_id: forward_id.into(),
            generation,
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelHeartbeatMeta {
    pub nonce: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardRecoveringMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub old_generation: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardRecoveredMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub generation: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardDropKind {
    TcpStream,
    UdpDatagram,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardDropMeta {
    pub kind: ForwardDropKind,
    pub count: u64,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ForwardDropMeta {
    pub fn new(
        kind: ForwardDropKind,
        count: u64,
        reason: impl Into<String>,
        message: Option<String>,
    ) -> Self {
        Self {
            kind,
            count,
            reason: reason.into(),
            message,
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelErrorMeta {
    #[serde(default)]
    code: String,
    #[serde(default = "default_tunnel_error_message")]
    pub message: String,
    #[serde(default)]
    pub fatal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

impl TunnelErrorMeta {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        fatal: bool,
        generation: Option<u64>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            fatal,
            generation,
        }
    }

    pub fn wire_code(&self) -> &str {
        &self.code
    }
}

fn default_tunnel_error_message() -> String {
    "port tunnel error".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EndpointMeta {
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TcpAcceptMeta {
    pub listener_stream_id: u32,
    #[serde(default)]
    pub peer: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct UdpDatagramMeta {
    pub peer: String,
}
