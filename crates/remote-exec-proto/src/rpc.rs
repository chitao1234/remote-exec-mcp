use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthCheckResponse {
    pub status: String,
    pub daemon_version: String,
    pub daemon_instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInfoResponse {
    pub target: String,
    pub daemon_version: String,
    pub daemon_instance_id: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_image_read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecStartRequest {
    pub cmd: String,
    pub workdir: Option<String>,
    pub shell: Option<String>,
    pub tty: bool,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
    pub login: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWriteRequest {
    pub daemon_session_id: String,
    pub chars: String,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecResponse {
    pub daemon_session_id: Option<String>,
    pub daemon_instance_id: String,
    pub running: bool,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub original_token_count: Option<u32>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ExecWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWarning {
    pub code: String,
    pub message: String,
}

impl ExecWarning {
    pub fn session_limit_approaching(target: &str) -> Self {
        Self {
            code: "exec_session_limit_approaching".to_string(),
            message: format!("Target `{target}` now has 60 open exec sessions."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyRequest {
    pub patch: String,
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyResponse {
    pub output: String,
}

pub const TRANSFER_SOURCE_TYPE_HEADER: &str = "x-remote-exec-source-type";
pub const TRANSFER_DESTINATION_PATH_HEADER: &str = "x-remote-exec-destination-path";
pub const TRANSFER_OVERWRITE_HEADER: &str = "x-remote-exec-overwrite";
pub const TRANSFER_CREATE_PARENT_HEADER: &str = "x-remote-exec-create-parent";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwriteMode {
    Fail,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferExportRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportRequest {
    pub destination_path: String,
    pub overwrite: TransferOverwriteMode,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportResponse {
    pub source_type: TransferSourceType,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub directories_copied: u64,
    pub replaced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadRequest {
    pub path: String,
    pub workdir: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadResponse {
    pub image_url: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorBody {
    pub code: String,
    pub message: String,
}
