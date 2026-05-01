use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rpc::{ExecWarning, TransferWarning};

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
pub enum TransferOverwrite {
    Fail,
    #[default]
    Merge,
    Replace,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferDestinationMode {
    #[default]
    Auto,
    Exact,
    IntoDirectory,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferSymlinkMode {
    #[default]
    Preserve,
    Follow,
    Skip,
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
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortStatus {
    Open,
    Closed,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::TransferSymlinkMode;

    #[test]
    fn transfer_symlink_mode_reject_is_unsupported() {
        let parsed = serde_json::from_str::<TransferSymlinkMode>("\"reject\"");

        assert!(parsed.is_err());
    }
}
