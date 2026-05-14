use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::WarningCode;
use crate::transfer::{
    TransferCompression, TransferExportMetadata, TransferImportMetadata, TransferOverwrite,
    TransferSourceType, TransferSymlinkMode,
};

pub const TRANSFER_SOURCE_TYPE_HEADER: &str = "x-remote-exec-source-type";
pub const TRANSFER_COMPRESSION_HEADER: &str = "x-remote-exec-compression";
pub const TRANSFER_DESTINATION_PATH_HEADER: &str = "x-remote-exec-destination-path";
pub const TRANSFER_OVERWRITE_HEADER: &str = "x-remote-exec-overwrite";
pub const TRANSFER_CREATE_PARENT_HEADER: &str = "x-remote-exec-create-parent";
pub const TRANSFER_SYMLINK_MODE_HEADER: &str = "x-remote-exec-symlink-mode";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferHeaders {
    pub destination_path: Option<String>,
    pub overwrite: Option<String>,
    pub create_parent: Option<String>,
    pub source_type: Option<String>,
    pub compression: Option<String>,
    pub symlink_mode: Option<String>,
}

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

pub fn transfer_destination_path_header_value(destination_path: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(destination_path.as_bytes())
}

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
            transfer_destination_path_header_value(&metadata.destination_path),
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
    headers: &TransferHeaders,
) -> Result<TransferExportMetadata, TransferHeaderError> {
    Ok(TransferExportMetadata {
        source_type: parse_required_source_type(headers)?,
        compression: parse_optional_compression(headers)?,
    })
}

pub fn parse_transfer_import_metadata(
    headers: &TransferHeaders,
) -> Result<TransferImportMetadata, TransferHeaderError> {
    Ok(TransferImportMetadata {
        destination_path: decode_transfer_destination_path_header(required_transfer_header(
            headers.destination_path.as_ref(),
            TRANSFER_DESTINATION_PATH_HEADER,
        )?)?,
        overwrite: parse_required_overwrite(headers)?,
        create_parent: parse_required_create_parent(headers)?,
        source_type: parse_required_source_type(headers)?,
        compression: parse_optional_compression(headers)?,
        symlink_mode: parse_optional_symlink_mode(headers)?,
    })
}

fn required_transfer_header(
    value: Option<&String>,
    name: &'static str,
) -> Result<String, TransferHeaderError> {
    let value = value
        .cloned()
        .ok_or_else(|| TransferHeaderError::missing(name))?;
    validate_transfer_header_value(&value, name)?;
    Ok(value)
}

fn optional_transfer_header(
    value: Option<&String>,
    name: &'static str,
) -> Result<Option<String>, TransferHeaderError> {
    match value {
        Some(value) => {
            validate_transfer_header_value(value, name)?;
            Ok(Some(value.clone()))
        }
        None => Ok(None),
    }
}

fn validate_transfer_header_value(
    value: &str,
    name: &'static str,
) -> Result<(), TransferHeaderError> {
    if value.contains('\n') || value.contains('\r') {
        Err(TransferHeaderError::invalid(
            name,
            "header value contains invalid newline characters",
        ))
    } else {
        Ok(())
    }
}

fn decode_transfer_destination_path_header(
    encoded: String,
) -> Result<String, TransferHeaderError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|err| {
            TransferHeaderError::invalid(
                TRANSFER_DESTINATION_PATH_HEADER,
                format!("expected base64-encoded UTF-8 path: {err}"),
            )
        })?;
    String::from_utf8(bytes).map_err(|err| {
        TransferHeaderError::invalid(
            TRANSFER_DESTINATION_PATH_HEADER,
            format!("expected base64-encoded UTF-8 path: {err}"),
        )
    })
}

fn parse_required_source_type(
    headers: &TransferHeaders,
) -> Result<TransferSourceType, TransferHeaderError> {
    let raw = required_transfer_header(headers.source_type.as_ref(), TRANSFER_SOURCE_TYPE_HEADER)?;
    TransferSourceType::from_wire_value(&raw).ok_or_else(|| {
        invalid_enum_header(
            TRANSFER_SOURCE_TYPE_HEADER,
            "expected one of `file`, `directory`, `multiple`",
        )
    })
}

fn parse_required_overwrite(
    headers: &TransferHeaders,
) -> Result<TransferOverwrite, TransferHeaderError> {
    let raw = required_transfer_header(headers.overwrite.as_ref(), TRANSFER_OVERWRITE_HEADER)?;
    TransferOverwrite::from_wire_value(&raw).ok_or_else(|| {
        invalid_enum_header(
            TRANSFER_OVERWRITE_HEADER,
            "expected one of `fail`, `merge`, `replace`",
        )
    })
}

fn parse_required_create_parent(headers: &TransferHeaders) -> Result<bool, TransferHeaderError> {
    match required_transfer_header(
        headers.create_parent.as_ref(),
        TRANSFER_CREATE_PARENT_HEADER,
    )?
    .as_str()
    {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(TransferHeaderError::invalid(
            TRANSFER_CREATE_PARENT_HEADER,
            "expected `true` or `false`",
        )),
    }
}

fn parse_optional_compression(
    headers: &TransferHeaders,
) -> Result<TransferCompression, TransferHeaderError> {
    optional_transfer_header(headers.compression.as_ref(), TRANSFER_COMPRESSION_HEADER)?
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

fn parse_optional_symlink_mode(
    headers: &TransferHeaders,
) -> Result<TransferSymlinkMode, TransferHeaderError> {
    optional_transfer_header(headers.symlink_mode.as_ref(), TRANSFER_SYMLINK_MODE_HEADER)?
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

#[cfg(test)]
mod tests {
    use super::{
        TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
        TRANSFER_SYMLINK_MODE_HEADER, TransferHeaderErrorKind, TransferHeaders,
        parse_transfer_export_metadata, parse_transfer_import_metadata,
        transfer_destination_path_header_value,
        transfer_export_header_pairs, transfer_import_header_pairs,
    };
    use crate::transfer::{
        TransferCompression, TransferExportMetadata, TransferImportMetadata, TransferOverwrite,
        TransferSourceType, TransferSymlinkMode,
    };

    fn transfer_headers(headers: &[(&'static str, &'static str)]) -> TransferHeaders {
        let mut values = TransferHeaders::default();
        for (name, value) in headers {
            match *name {
                TRANSFER_DESTINATION_PATH_HEADER => {
                    values.destination_path = Some((*value).to_string())
                }
                TRANSFER_OVERWRITE_HEADER => values.overwrite = Some((*value).to_string()),
                TRANSFER_CREATE_PARENT_HEADER => values.create_parent = Some((*value).to_string()),
                TRANSFER_SOURCE_TYPE_HEADER => values.source_type = Some((*value).to_string()),
                TRANSFER_COMPRESSION_HEADER => values.compression = Some((*value).to_string()),
                TRANSFER_SYMLINK_MODE_HEADER => values.symlink_mode = Some((*value).to_string()),
                _ => panic!("unexpected transfer header name `{name}`"),
            }
        }
        values
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
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    transfer_destination_path_header_value("/tmp/output"),
                ),
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
        let headers = transfer_headers(&[
            (
                TRANSFER_DESTINATION_PATH_HEADER,
                "L3RtcC9vdXRwdXQ=",
            ),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let parsed = parse_transfer_import_metadata(&headers).unwrap();

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
            let mut headers = transfer_headers(&[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    "L3RtcC9vdXRwdXQ=",
                ),
                (TRANSFER_OVERWRITE_HEADER, "merge"),
                (TRANSFER_CREATE_PARENT_HEADER, "true"),
                (TRANSFER_SOURCE_TYPE_HEADER, "file"),
            ]);
            match missing {
                TRANSFER_DESTINATION_PATH_HEADER => headers.destination_path = None,
                TRANSFER_OVERWRITE_HEADER => headers.overwrite = None,
                TRANSFER_CREATE_PARENT_HEADER => headers.create_parent = None,
                TRANSFER_SOURCE_TYPE_HEADER => headers.source_type = None,
                _ => unreachable!(),
            }

            let err = parse_transfer_import_metadata(&headers).unwrap_err();

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
            let mut headers = transfer_headers(&[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    "L3RtcC9vdXRwdXQ=",
                ),
                (TRANSFER_OVERWRITE_HEADER, "merge"),
                (TRANSFER_CREATE_PARENT_HEADER, "true"),
                (TRANSFER_SOURCE_TYPE_HEADER, "file"),
                (TRANSFER_COMPRESSION_HEADER, "none"),
                (TRANSFER_SYMLINK_MODE_HEADER, "preserve"),
            ]);
            match header {
                TRANSFER_OVERWRITE_HEADER => headers.overwrite = Some(value.to_string()),
                TRANSFER_CREATE_PARENT_HEADER => headers.create_parent = Some(value.to_string()),
                TRANSFER_SOURCE_TYPE_HEADER => headers.source_type = Some(value.to_string()),
                TRANSFER_COMPRESSION_HEADER => headers.compression = Some(value.to_string()),
                TRANSFER_SYMLINK_MODE_HEADER => headers.symlink_mode = Some(value.to_string()),
                _ => unreachable!(),
            }

            let err = parse_transfer_import_metadata(&headers).unwrap_err();

            assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
            assert_eq!(err.header, header);
        }
    }

    #[test]
    fn transfer_header_parser_rejects_destination_path_with_newlines() {
        let headers = transfer_headers(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output\r\nx-injected: nope"),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let err = parse_transfer_import_metadata(&headers).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_DESTINATION_PATH_HEADER);
    }

    #[test]
    fn transfer_header_parser_round_trips_unicode_destination_path() {
        let headers = transfer_headers(&[
            (
                TRANSFER_DESTINATION_PATH_HEADER,
                "L3RtcC/mtYvor5Uv0L/RgNC40LLQtdGCL3LDqXN1bcOpLnR4dA==",
            ),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let parsed = parse_transfer_import_metadata(&headers).unwrap();

        assert_eq!(parsed.destination_path, "/tmp/测试/привет/résumé.txt");
    }

    #[test]
    fn transfer_header_parser_rejects_non_base64_destination_path() {
        let headers = transfer_headers(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let err = parse_transfer_import_metadata(&headers).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_DESTINATION_PATH_HEADER);
    }

    #[test]
    fn transfer_header_parser_reads_export_metadata_defaults() {
        let headers = transfer_headers(&[(TRANSFER_SOURCE_TYPE_HEADER, "directory")]);

        let parsed = parse_transfer_export_metadata(&headers).unwrap();

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
        let missing = TransferHeaders::default();
        let err = parse_transfer_export_metadata(&missing).unwrap_err();
        assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);

        let invalid = transfer_headers(&[(TRANSFER_SOURCE_TYPE_HEADER, "folder")]);
        let err = parse_transfer_export_metadata(&invalid).unwrap_err();
        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);
    }

    #[test]
    fn transfer_header_parser_rejects_header_values_with_newlines() {
        let headers = TransferHeaders {
            source_type: Some("directory\nremote".to_string()),
            ..TransferHeaders::default()
        };

        let err = parse_transfer_export_metadata(&headers).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);
    }
}
