use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rpc::{
    DaemonIdentity, ExecPtySize, ExecWarning, TargetCapabilities, TargetInfoResponse,
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
    #[serde(flatten)]
    pub capabilities: TargetCapabilities,
}

impl From<&TargetInfoResponse> for ListTargetDaemonInfo {
    fn from(value: &TargetInfoResponse) -> Self {
        Self {
            identity: value.identity.clone(),
            capabilities: value.capabilities.clone(),
        }
    }
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
