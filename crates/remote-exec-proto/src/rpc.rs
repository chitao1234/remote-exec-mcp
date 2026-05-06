use schemars::JsonSchema;
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
    #[serde(default)]
    pub supports_transfer_compression: bool,
    #[serde(default)]
    pub supports_port_forward: bool,
    #[serde(default)]
    pub port_forward_protocol_version: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferCompression {
    #[default]
    None,
    Zstd,
}

impl TransferCompression {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Zstd => "zstd",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
    Multiple,
}

impl TransferSourceType {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Multiple => "multiple",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "file" => Some(Self::File),
            "directory" => Some(Self::Directory),
            "multiple" => Some(Self::Multiple),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwriteMode {
    Fail,
    Merge,
    Replace,
}

impl TransferOverwriteMode {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Merge => "merge",
            Self::Replace => "replace",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "fail" => Some(Self::Fail),
            "merge" => Some(Self::Merge),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSymlinkMode {
    #[default]
    Preserve,
    Follow,
    Skip,
}

impl TransferSymlinkMode {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Follow => "follow",
            Self::Skip => "skip",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "preserve" => Some(Self::Preserve),
            "follow" => Some(Self::Follow),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TransferWarning {
    pub code: String,
    pub message: String,
}

impl TransferWarning {
    pub fn skipped_unsupported_entry(path: impl std::fmt::Display) -> Self {
        Self {
            code: "transfer_skipped_unsupported_entry".to_string(),
            message: format!("Skipped unsupported transfer source entry `{path}`."),
        }
    }

    pub fn skipped_symlink(path: impl std::fmt::Display) -> Self {
        Self {
            code: "transfer_skipped_symlink".to_string(),
            message: format!("Skipped symlink transfer source entry `{path}`."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferExportRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "TransferCompression::is_none")]
    pub compression: TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExportMetadata {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportRequest {
    pub destination_path: String,
    pub overwrite: TransferOverwriteMode,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferImportMetadata {
    pub destination_path: String,
    pub overwrite: TransferOverwriteMode,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub symlink_mode: TransferSymlinkMode,
}

impl From<&TransferImportRequest> for TransferImportMetadata {
    fn from(value: &TransferImportRequest) -> Self {
        Self {
            destination_path: value.destination_path.clone(),
            overwrite: value.overwrite.clone(),
            create_parent: value.create_parent,
            source_type: value.source_type.clone(),
            compression: value.compression.clone(),
            symlink_mode: value.symlink_mode.clone(),
        }
    }
}

impl From<TransferImportMetadata> for TransferImportRequest {
    fn from(value: TransferImportMetadata) -> Self {
        Self {
            destination_path: value.destination_path,
            overwrite: value.overwrite,
            create_parent: value.create_parent,
            source_type: value.source_type,
            compression: value.compression,
            symlink_mode: value.symlink_mode,
        }
    }
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

fn parse_required_overwrite<F>(header: &mut F) -> Result<TransferOverwriteMode, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    let raw = required_transfer_header(header, TRANSFER_OVERWRITE_HEADER)?;
    TransferOverwriteMode::from_wire_value(&raw).ok_or_else(|| {
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
#[serde(rename_all = "snake_case")]
pub enum PortForwardProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortForwardLease {
    pub lease_id: String,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortListenRequest {
    pub endpoint: String,
    pub protocol: PortForwardProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<PortForwardLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortListenResponse {
    pub bind_id: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortListenAcceptRequest {
    pub bind_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortListenAcceptResponse {
    pub connection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConnectRequest {
    pub endpoint: String,
    pub protocol: PortForwardProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<PortForwardLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConnectResponse {
    pub connection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConnectionReadRequest {
    pub connection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConnectionReadResponse {
    pub data: String,
    pub eof: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConnectionWriteRequest {
    pub connection_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortUdpDatagramReadRequest {
    pub bind_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortUdpDatagramReadResponse {
    pub peer: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortUdpDatagramWriteRequest {
    pub bind_id: String,
    pub peer: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortConnectionCloseRequest {
    pub connection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortListenCloseRequest {
    pub bind_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortLeaseRenewRequest {
    pub lease_id: String,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmptyResponse {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorBody {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ExecStartRequest, ExecWriteRequest, ImageReadRequest, PatchApplyRequest,
        TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
        TRANSFER_SYMLINK_MODE_HEADER, TransferCompression, TransferExportMetadata,
        TransferHeaderError, TransferHeaderErrorKind, TransferImportMetadata,
        TransferOverwriteMode, TransferSourceType, TransferSymlinkMode,
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
    fn port_forward_requests_omit_optional_lease() {
        let listen = super::PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: super::PortForwardProtocol::Tcp,
            lease: None,
        };
        let connect = super::PortConnectRequest {
            endpoint: "127.0.0.1:80".to_string(),
            protocol: super::PortForwardProtocol::Tcp,
            lease: None,
        };

        assert_eq!(
            serde_json::to_value(&listen).unwrap(),
            serde_json::json!({
                "endpoint": "127.0.0.1:0",
                "protocol": "tcp",
            })
        );
        assert_eq!(
            serde_json::to_value(&connect).unwrap(),
            serde_json::json!({
                "endpoint": "127.0.0.1:80",
                "protocol": "tcp",
            })
        );
    }

    #[test]
    fn transfer_header_pairs_render_canonical_import_metadata() {
        let metadata = TransferImportMetadata {
            destination_path: "/tmp/output".to_string(),
            overwrite: TransferOverwriteMode::Replace,
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
                overwrite: TransferOverwriteMode::Merge,
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
