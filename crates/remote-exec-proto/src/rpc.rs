use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod error;

pub use error::{RpcErrorBody, RpcErrorCode};

pub use crate::transfer::{
    TransferCompression, TransferExportMetadata, TransferExportRequest, TransferImportMetadata,
    TransferImportRequest, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};

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
    #[serde(default)]
    pub supports_transfer_compression: bool,
    #[serde(default)]
    pub supports_port_forward: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_forward_protocol_version: Option<PortForwardProtocolVersion>,
}

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct PortForwardProtocolVersion(NonZeroU32);

impl PortForwardProtocolVersion {
    pub fn v4() -> Self {
        Self(NonZeroU32::new(4).expect("v4 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecStartRequest {
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    pub tty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yield_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWriteRequest {
    pub daemon_session_id: String,
    pub chars: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yield_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecOutputResponse {
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

#[derive(Debug, Clone, PartialEq)]
pub struct ExecRunningResponse {
    pub daemon_session_id: String,
    pub output: ExecOutputResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecCompletedResponse {
    pub output: ExecOutputResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecResponse {
    Running(ExecRunningResponse),
    Completed(ExecCompletedResponse),
}

#[derive(Debug, Serialize, Deserialize)]
struct ExecResponseWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daemon_session_id: Option<String>,
    daemon_instance_id: String,
    running: bool,
    chunk_id: Option<String>,
    wall_time_seconds: f64,
    exit_code: Option<i32>,
    original_token_count: Option<u32>,
    output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<ExecWarning>,
}

impl ExecResponseWire {
    fn output(self) -> ExecOutputResponse {
        ExecOutputResponse {
            daemon_instance_id: self.daemon_instance_id,
            running: self.running,
            chunk_id: self.chunk_id,
            wall_time_seconds: self.wall_time_seconds,
            exit_code: self.exit_code,
            original_token_count: self.original_token_count,
            output: self.output,
            warnings: self.warnings,
        }
    }
}

impl From<ExecResponse> for ExecResponseWire {
    fn from(response: ExecResponse) -> Self {
        match response {
            ExecResponse::Running(response) => {
                let output = response.output;
                Self {
                    daemon_session_id: Some(response.daemon_session_id),
                    daemon_instance_id: output.daemon_instance_id,
                    running: true,
                    chunk_id: output.chunk_id,
                    wall_time_seconds: output.wall_time_seconds,
                    exit_code: output.exit_code,
                    original_token_count: output.original_token_count,
                    output: output.output,
                    warnings: output.warnings,
                }
            }
            ExecResponse::Completed(response) => {
                let output = response.output;
                Self {
                    daemon_session_id: None,
                    daemon_instance_id: output.daemon_instance_id,
                    running: false,
                    chunk_id: output.chunk_id,
                    wall_time_seconds: output.wall_time_seconds,
                    exit_code: output.exit_code,
                    original_token_count: output.original_token_count,
                    output: output.output,
                    warnings: output.warnings,
                }
            }
        }
    }
}

impl Serialize for ExecResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ExecResponseWire::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExecResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ExecResponseWire::deserialize(deserializer)?;
        let running = wire.running;
        let daemon_session_id = wire.daemon_session_id.clone();
        match (running, daemon_session_id) {
            (true, Some(daemon_session_id)) if !daemon_session_id.is_empty() => {
                Ok(Self::Running(ExecRunningResponse {
                    daemon_session_id,
                    output: wire.output(),
                }))
            }
            (true, _) => Err(serde::de::Error::custom(
                "daemon returned malformed exec response: running response missing daemon_session_id",
            )),
            (false, None) => Ok(Self::Completed(ExecCompletedResponse {
                output: wire.output(),
            })),
            (false, Some(_)) => Err(serde::de::Error::custom(
                "daemon returned malformed exec response: completed response unexpectedly included daemon_session_id",
            )),
        }
    }
}

impl ExecResponse {
    pub fn running(&self) -> bool {
        self.output().running
    }

    pub fn output(&self) -> &ExecOutputResponse {
        match self {
            Self::Running(response) => &response.output,
            Self::Completed(response) => &response.output,
        }
    }

    pub fn daemon_session_id(&self) -> Option<&str> {
        match self {
            Self::Running(response) => Some(response.daemon_session_id.as_str()),
            Self::Completed(_) => None,
        }
    }

    pub fn into_output(self) -> ExecOutputResponse {
        match self {
            Self::Running(response) => response.output,
            Self::Completed(response) => response.output,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecStartResponse {
    pub daemon_session_id: String,
    pub response: ExecResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecWriteResponse {
    pub response: ExecResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExecWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    ExecSessionLimitApproaching,
    TransferSkippedUnsupportedEntry,
    TransferSkippedSymlink,
}

impl WarningCode {
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::ExecSessionLimitApproaching => "exec_session_limit_approaching",
            Self::TransferSkippedUnsupportedEntry => "transfer_skipped_unsupported_entry",
            Self::TransferSkippedSymlink => "transfer_skipped_symlink",
        }
    }
}

impl ExecWarning {
    pub fn session_limit_approaching(target: &str) -> Self {
        Self {
            code: WarningCode::ExecSessionLimitApproaching
                .wire_value()
                .to_string(),
            message: format!("Target `{target}` now has 60 open exec sessions."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyRequest {
    pub patch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyResponse {
    pub output: String,
}

pub const TRANSFER_SOURCE_TYPE_HEADER: &str = "x-remote-exec-source-type";
pub const TRANSFER_COMPRESSION_HEADER: &str = "x-remote-exec-compression";
pub const TRANSFER_DESTINATION_PATH_HEADER: &str = "x-remote-exec-destination-path";
pub const TRANSFER_OVERWRITE_HEADER: &str = "x-remote-exec-overwrite";
pub const TRANSFER_CREATE_PARENT_HEADER: &str = "x-remote-exec-create-parent";
pub const TRANSFER_SYMLINK_MODE_HEADER: &str = "x-remote-exec-symlink-mode";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TransferWarning {
    pub code: String,
    pub message: String,
}

impl TransferWarning {
    pub fn skipped_unsupported_entry(path: impl std::fmt::Display) -> Self {
        Self {
            code: WarningCode::TransferSkippedUnsupportedEntry
                .wire_value()
                .to_string(),
            message: format!("Skipped unsupported transfer source entry `{path}`."),
        }
    }

    pub fn skipped_symlink(path: impl std::fmt::Display) -> Self {
        Self {
            code: WarningCode::TransferSkippedSymlink.wire_value().to_string(),
            message: format!("Skipped symlink transfer source entry `{path}`."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferPathInfoRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferPathInfoResponse {
    pub exists: bool,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferHeaderErrorKind {
    Missing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferHeaderError {
    pub header: &'static str,
    pub kind: TransferHeaderErrorKind,
    pub message: String,
}

impl TransferHeaderError {
    pub fn missing(header: &'static str) -> Self {
        Self {
            header,
            kind: TransferHeaderErrorKind::Missing,
            message: format!("missing header `{header}`"),
        }
    }

    pub fn invalid(header: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            header,
            kind: TransferHeaderErrorKind::Invalid,
            message: format!("invalid header `{header}`: {message}"),
        }
    }
}

impl std::fmt::Display for TransferHeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransferHeaderError {}

pub type TransferHeaderPairs = Vec<(&'static str, String)>;

pub fn transfer_export_header_pairs(metadata: &TransferExportMetadata) -> TransferHeaderPairs {
    vec![
        (
            TRANSFER_SOURCE_TYPE_HEADER,
            metadata.source_type.wire_value().to_string(),
        ),
        (
            TRANSFER_COMPRESSION_HEADER,
            metadata.compression.wire_value().to_string(),
        ),
    ]
}

pub fn transfer_import_header_pairs(metadata: &TransferImportMetadata) -> TransferHeaderPairs {
    vec![
        (
            TRANSFER_DESTINATION_PATH_HEADER,
            metadata.destination_path.clone(),
        ),
        (
            TRANSFER_OVERWRITE_HEADER,
            metadata.overwrite.wire_value().to_string(),
        ),
        (
            TRANSFER_CREATE_PARENT_HEADER,
            metadata.create_parent.to_string(),
        ),
        (
            TRANSFER_SOURCE_TYPE_HEADER,
            metadata.source_type.wire_value().to_string(),
        ),
        (
            TRANSFER_COMPRESSION_HEADER,
            metadata.compression.wire_value().to_string(),
        ),
        (
            TRANSFER_SYMLINK_MODE_HEADER,
            metadata.symlink_mode.wire_value().to_string(),
        ),
    ]
}

pub fn parse_transfer_export_metadata(
    mut header: impl FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
) -> Result<TransferExportMetadata, TransferHeaderError> {
    Ok(TransferExportMetadata {
        source_type: parse_required_source_type(&mut header)?,
        compression: parse_optional_compression(&mut header)?,
    })
}

pub fn parse_transfer_import_metadata(
    mut header: impl FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
) -> Result<TransferImportMetadata, TransferHeaderError> {
    Ok(TransferImportMetadata {
        destination_path: required_transfer_header(&mut header, TRANSFER_DESTINATION_PATH_HEADER)?,
        overwrite: parse_required_overwrite(&mut header)?,
        create_parent: parse_required_create_parent(&mut header)?,
        source_type: parse_required_source_type(&mut header)?,
        compression: parse_optional_compression(&mut header)?,
        symlink_mode: parse_optional_symlink_mode(&mut header)?,
    })
}

fn required_transfer_header<F>(
    header: &mut F,
    name: &'static str,
) -> Result<String, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    optional_transfer_header(header, name)?.ok_or_else(|| TransferHeaderError::missing(name))
}

fn optional_transfer_header<F>(
    header: &mut F,
    name: &'static str,
) -> Result<Option<String>, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    header(name)
}

fn parse_required_source_type<F>(header: &mut F) -> Result<TransferSourceType, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    let raw = required_transfer_header(header, TRANSFER_SOURCE_TYPE_HEADER)?;
    TransferSourceType::from_wire_value(&raw).ok_or_else(|| {
        invalid_enum_header(
            TRANSFER_SOURCE_TYPE_HEADER,
            "expected one of `file`, `directory`, `multiple`",
        )
    })
}

fn parse_required_overwrite<F>(header: &mut F) -> Result<TransferOverwrite, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    let raw = required_transfer_header(header, TRANSFER_OVERWRITE_HEADER)?;
    TransferOverwrite::from_wire_value(&raw).ok_or_else(|| {
        invalid_enum_header(
            TRANSFER_OVERWRITE_HEADER,
            "expected one of `fail`, `merge`, `replace`",
        )
    })
}

fn parse_required_create_parent<F>(header: &mut F) -> Result<bool, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    match required_transfer_header(header, TRANSFER_CREATE_PARENT_HEADER)?.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(TransferHeaderError::invalid(
            TRANSFER_CREATE_PARENT_HEADER,
            "expected `true` or `false`",
        )),
    }
}

fn parse_optional_compression<F>(header: &mut F) -> Result<TransferCompression, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    optional_transfer_header(header, TRANSFER_COMPRESSION_HEADER)?
        .as_deref()
        .map(|raw| {
            TransferCompression::from_wire_value(raw).ok_or_else(|| {
                invalid_enum_header(
                    TRANSFER_COMPRESSION_HEADER,
                    "expected one of `none`, `zstd`",
                )
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse_optional_symlink_mode<F>(
    header: &mut F,
) -> Result<TransferSymlinkMode, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    optional_transfer_header(header, TRANSFER_SYMLINK_MODE_HEADER)?
        .as_deref()
        .map(|raw| {
            TransferSymlinkMode::from_wire_value(raw).ok_or_else(|| {
                invalid_enum_header(
                    TRANSFER_SYMLINK_MODE_HEADER,
                    "expected one of `preserve`, `follow`, `skip`",
                )
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn invalid_enum_header(header: &'static str, message: &'static str) -> TransferHeaderError {
    TransferHeaderError::invalid(header, message)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportResponse {
    pub source_type: TransferSourceType,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub directories_copied: u64,
    pub replaced: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<TransferWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadRequest {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadResponse {
    pub image_url: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmptyResponse {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecStartRequest,
        ExecWriteRequest, ImageReadRequest, PatchApplyRequest, TRANSFER_COMPRESSION_HEADER,
        TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER,
        TRANSFER_SOURCE_TYPE_HEADER, TRANSFER_SYMLINK_MODE_HEADER, TransferCompression,
        TransferExportMetadata, TransferHeaderError, TransferHeaderErrorKind,
        TransferImportMetadata, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
        parse_transfer_export_metadata, parse_transfer_import_metadata,
        transfer_export_header_pairs, transfer_import_header_pairs,
    };

    fn header_map(headers: &[(&'static str, &'static str)]) -> BTreeMap<&'static str, String> {
        headers
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect()
    }

    fn lookup<'a>(
        headers: &'a BTreeMap<&'static str, String>,
    ) -> impl FnMut(&'static str) -> Result<Option<String>, TransferHeaderError> + 'a {
        move |name| Ok(headers.get(name).cloned())
    }

    #[test]
    fn exec_start_request_omits_none_fields() {
        let request = ExecStartRequest {
            cmd: "echo hi".to_string(),
            workdir: None,
            shell: None,
            tty: false,
            yield_time_ms: None,
            max_output_tokens: None,
            login: None,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "cmd": "echo hi",
                "tty": false,
            })
        );
    }

    #[test]
    fn exec_write_request_omits_none_fields() {
        let request = ExecWriteRequest {
            daemon_session_id: "daemon-session-1".to_string(),
            chars: String::new(),
            yield_time_ms: None,
            max_output_tokens: None,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "daemon_session_id": "daemon-session-1",
                "chars": "",
            })
        );
    }

    #[test]
    fn running_exec_response_requires_daemon_session_id() {
        let value = serde_json::json!({
            "daemon_instance_id": "inst",
            "running": true,
            "chunk_id": "chunk",
            "wall_time_seconds": 0.1,
            "exit_code": null,
            "original_token_count": 1,
            "output": "hi"
        });

        assert!(serde_json::from_value::<ExecResponse>(value).is_err());
    }

    #[test]
    fn completed_exec_response_rejects_daemon_session_id() {
        let value = serde_json::json!({
            "daemon_session_id": "daemon-session-1",
            "daemon_instance_id": "inst",
            "running": false,
            "chunk_id": null,
            "wall_time_seconds": 0.1,
            "exit_code": 0,
            "original_token_count": 1,
            "output": "done"
        });

        assert!(serde_json::from_value::<ExecResponse>(value).is_err());
    }

    #[test]
    fn completed_exec_response_omits_daemon_session_id() {
        let response = ExecResponse::Completed(ExecCompletedResponse {
            output: ExecOutputResponse {
                daemon_instance_id: "inst".to_string(),
                running: false,
                chunk_id: None,
                wall_time_seconds: 0.1,
                exit_code: Some(0),
                original_token_count: Some(1),
                output: "done".to_string(),
                warnings: Vec::new(),
            },
        });

        let value = serde_json::to_value(response).unwrap();

        assert!(value.get("daemon_session_id").is_none());
    }

    #[test]
    fn patch_and_image_requests_omit_none_fields() {
        let patch = PatchApplyRequest {
            patch: "*** Begin Patch\n*** End Patch\n".to_string(),
            workdir: None,
        };
        let image = ImageReadRequest {
            path: "image.png".to_string(),
            workdir: None,
            detail: None,
        };

        assert_eq!(
            serde_json::to_value(&patch).unwrap(),
            serde_json::json!({
                "patch": "*** Begin Patch\n*** End Patch\n",
            })
        );
        assert_eq!(
            serde_json::to_value(&image).unwrap(),
            serde_json::json!({
                "path": "image.png",
            })
        );
    }

    #[test]
    fn transfer_header_pairs_render_canonical_import_metadata() {
        let metadata = TransferImportMetadata {
            destination_path: "/tmp/output".to_string(),
            overwrite: TransferOverwrite::Replace,
            create_parent: true,
            source_type: TransferSourceType::Directory,
            compression: TransferCompression::Zstd,
            symlink_mode: TransferSymlinkMode::Follow,
        };

        assert_eq!(
            transfer_import_header_pairs(&metadata),
            vec![
                (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output".to_string()),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "zstd".to_string()),
                (TRANSFER_SYMLINK_MODE_HEADER, "follow".to_string()),
            ]
        );
    }

    #[test]
    fn transfer_header_pairs_render_canonical_export_metadata() {
        let metadata = TransferExportMetadata {
            source_type: TransferSourceType::Multiple,
            compression: TransferCompression::None,
        };

        assert_eq!(
            transfer_export_header_pairs(&metadata),
            vec![
                (TRANSFER_SOURCE_TYPE_HEADER, "multiple".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
            ]
        );
    }

    #[test]
    fn transfer_header_parser_reads_import_metadata_and_optional_defaults() {
        let headers = header_map(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let parsed = parse_transfer_import_metadata(lookup(&headers)).unwrap();

        assert_eq!(
            parsed,
            TransferImportMetadata {
                destination_path: "/tmp/output".to_string(),
                overwrite: TransferOverwrite::Merge,
                create_parent: false,
                source_type: TransferSourceType::File,
                compression: TransferCompression::None,
                symlink_mode: TransferSymlinkMode::Preserve,
            }
        );
    }

    #[test]
    fn transfer_header_parser_rejects_missing_required_import_headers() {
        for missing in [
            TRANSFER_DESTINATION_PATH_HEADER,
            TRANSFER_OVERWRITE_HEADER,
            TRANSFER_CREATE_PARENT_HEADER,
            TRANSFER_SOURCE_TYPE_HEADER,
        ] {
            let mut headers = header_map(&[
                (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
                (TRANSFER_OVERWRITE_HEADER, "merge"),
                (TRANSFER_CREATE_PARENT_HEADER, "true"),
                (TRANSFER_SOURCE_TYPE_HEADER, "file"),
            ]);
            headers.remove(missing);

            let err = parse_transfer_import_metadata(lookup(&headers)).unwrap_err();

            assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
            assert_eq!(err.header, missing);
        }
    }

    #[test]
    fn transfer_header_parser_rejects_invalid_import_metadata_values() {
        for (header, value) in [
            (TRANSFER_OVERWRITE_HEADER, "clobber"),
            (TRANSFER_CREATE_PARENT_HEADER, "yes"),
            (TRANSFER_SOURCE_TYPE_HEADER, "folder"),
            (TRANSFER_COMPRESSION_HEADER, "gzip"),
            (TRANSFER_SYMLINK_MODE_HEADER, "copy"),
        ] {
            let mut headers = header_map(&[
                (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
                (TRANSFER_OVERWRITE_HEADER, "merge"),
                (TRANSFER_CREATE_PARENT_HEADER, "true"),
                (TRANSFER_SOURCE_TYPE_HEADER, "file"),
                (TRANSFER_COMPRESSION_HEADER, "none"),
                (TRANSFER_SYMLINK_MODE_HEADER, "preserve"),
            ]);
            headers.insert(header, value.to_string());

            let err = parse_transfer_import_metadata(lookup(&headers)).unwrap_err();

            assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
            assert_eq!(err.header, header);
        }
    }

    #[test]
    fn transfer_header_parser_reads_export_metadata_defaults() {
        let headers = header_map(&[(TRANSFER_SOURCE_TYPE_HEADER, "directory")]);

        let parsed = parse_transfer_export_metadata(lookup(&headers)).unwrap();

        assert_eq!(
            parsed,
            TransferExportMetadata {
                source_type: TransferSourceType::Directory,
                compression: TransferCompression::None,
            }
        );
    }

    #[test]
    fn transfer_header_parser_rejects_invalid_export_metadata() {
        let missing = BTreeMap::new();
        let err = parse_transfer_export_metadata(lookup(&missing)).unwrap_err();
        assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);

        let invalid = header_map(&[(TRANSFER_SOURCE_TYPE_HEADER, "folder")]);
        let err = parse_transfer_export_metadata(lookup(&invalid)).unwrap_err();
        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);
    }
}
