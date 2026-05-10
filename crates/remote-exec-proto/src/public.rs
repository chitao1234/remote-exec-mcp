use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rpc::{ExecWarning, TransferWarning};
pub use crate::transfer::{TransferOverwrite, TransferSourceType, TransferSymlinkMode};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecCommandInput {
    pub target: String,
    pub cmd: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub login: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTargetsInput {}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteStdinInput {
    pub session_id: String,
    #[serde(default)]
    pub chars: Option<String>,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandToolResult {
    pub target: String,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub session_id: Option<String>,
    pub session_command: Option<String>,
    pub original_token_count: Option<u32>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ExecWarning>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_port_forward: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_forward_protocol_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetEntry {
    pub name: String,
    pub daemon_info: Option<ListTargetDaemonInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetsResult {
    pub targets: Vec<ListTargetEntry>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferDestinationMode {
    #[default]
    Auto,
    Exact,
    IntoDirectory,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferEndpoint {
    pub target: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferFilesInput {
    #[serde(default)]
    pub source: Option<TransferEndpoint>,
    #[serde(default)]
    pub sources: Vec<TransferEndpoint>,
    pub destination: TransferEndpoint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub overwrite: TransferOverwrite,
    #[serde(default)]
    pub destination_mode: TransferDestinationMode,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    pub create_parent: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TransferFilesResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<TransferEndpoint>,
    pub sources: Vec<TransferEndpoint>,
    pub destination: TransferEndpoint,
    pub resolved_destination: TransferEndpoint,
    pub destination_mode: TransferDestinationMode,
    pub symlink_mode: TransferSymlinkMode,
    pub source_type: TransferSourceType,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub directories_copied: u64,
    pub replaced: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<TransferWarning>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchInput {
    pub target: String,
    pub input: String,
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewImageInput {
    pub target: String,
    pub path: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ViewImageResult {
    pub target: String,
    pub image_url: String,
    pub detail: Option<String>,
}

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

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ForwardPortsResult {
    pub action: ForwardPortsAction,
    pub forwards: Vec<ForwardPortEntry>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
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
    pub last_reconnect_at: Option<String>,
    pub limits: ForwardPortLimitSummary,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_port_entry_serializes_additive_v4_state() {
        let entry = ForwardPortEntry {
            forward_id: "fwd_test".to_string(),
            listen_side: "local".to_string(),
            listen_endpoint: "127.0.0.1:10000".to_string(),
            connect_side: "builder-a".to_string(),
            connect_endpoint: "127.0.0.1:10001".to_string(),
            protocol: ForwardPortProtocol::Tcp,
            status: ForwardPortStatus::Open,
            last_error: None,
            phase: ForwardPortPhase::Reconnecting,
            listen_state: ForwardPortSideState {
                side: "local".to_string(),
                role: ForwardPortSideRole::Listen,
                generation: 2,
                health: ForwardPortSideHealth::Ready,
                last_error: None,
            },
            connect_state: ForwardPortSideState {
                side: "builder-a".to_string(),
                role: ForwardPortSideRole::Connect,
                generation: 3,
                health: ForwardPortSideHealth::Reconnecting,
                last_error: Some("transport loss".to_string()),
            },
            active_tcp_streams: 1,
            dropped_tcp_streams: 2,
            dropped_udp_datagrams: 3,
            reconnect_attempts: 4,
            last_reconnect_at: Some("2026-05-08T00:00:00Z".to_string()),
            limits: ForwardPortLimitSummary {
                max_active_tcp_streams: 256,
                max_udp_peers: 256,
                max_pending_tcp_bytes_per_stream: 262144,
                max_pending_tcp_bytes_per_forward: 2097152,
                max_tunnel_queued_bytes: 8388608,
                max_reconnecting_forwards: 16,
            },
        };

        let value = serde_json::to_value(entry).unwrap();
        assert_eq!(value["phase"], "reconnecting");
        assert_eq!(value["connect_state"]["health"], "reconnecting");
        assert_eq!(value["dropped_tcp_streams"], 2);
        assert_eq!(value["limits"]["max_tunnel_queued_bytes"], 8388608);
        assert_eq!(value["limits"]["max_reconnecting_forwards"], 16);
    }
}
