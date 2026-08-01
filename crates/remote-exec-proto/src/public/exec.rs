use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rpc::{
    DaemonIdentity, ExecPtySize, ExecWarning, FileToolProtocolVersion, TargetCapabilities,
    TargetInfoResponse, TransferStreamProtocolVersion,
};

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
    pub pty_size: Option<ExecPtySize>,
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
    #[serde(flatten)]
    pub identity: DaemonIdentity,
    pub supports_pty: bool,
    pub supports_port_forward: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_stream_protocol_version: Option<TransferStreamProtocolVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_tool_protocol_version: Option<FileToolProtocolVersion>,
}

impl From<&TargetInfoResponse> for ListTargetDaemonInfo {
    fn from(value: &TargetInfoResponse) -> Self {
        Self::from_identity_and_capabilities(value.identity.clone(), &value.capabilities)
    }
}

impl ListTargetDaemonInfo {
    pub fn from_identity_and_capabilities(
        identity: DaemonIdentity,
        capabilities: &TargetCapabilities,
    ) -> Self {
        Self {
            identity,
            supports_pty: capabilities.supports_pty,
            supports_port_forward: capabilities.supports_compatible_port_forward(),
            transfer_stream_protocol_version: capabilities.transfer_stream_protocol_version,
            file_tool_protocol_version: capabilities.file_tool_protocol_version,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetHealthStatus {
    Unknown,
    Healthy,
    MaybeUnhealthy,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetEntry {
    pub name: String,
    pub healthy: bool,
    pub health_status: TargetHealthStatus,
    pub daemon_info: Option<ListTargetDaemonInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetsResult {
    pub targets: Vec<ListTargetEntry>,
}
