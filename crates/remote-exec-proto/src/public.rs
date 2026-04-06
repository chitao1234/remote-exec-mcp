use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rpc::ExecWarning;

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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwrite {
    Fail,
    Replace,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
    Multiple,
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
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TransferFilesResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<TransferEndpoint>,
    pub sources: Vec<TransferEndpoint>,
    pub destination: TransferEndpoint,
    pub source_type: TransferSourceType,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub directories_copied: u64,
    pub replaced: bool,
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
