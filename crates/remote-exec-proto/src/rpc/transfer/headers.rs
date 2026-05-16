use base64::Engine;

use crate::transfer::{TransferExportMetadata, TransferImportMetadata};

use super::types::{TransferHeaderError, TransferHeaderErrorKind};

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

impl TransferHeaders {
    pub fn from_lookup<F>(mut lookup: F) -> Result<Self, TransferHeaderError>
    where
        F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
    {
        Ok(Self {
            destination_path: lookup(TRANSFER_DESTINATION_PATH_HEADER)?,
            overwrite: lookup(TRANSFER_OVERWRITE_HEADER)?,
            create_parent: lookup(TRANSFER_CREATE_PARENT_HEADER)?,
            source_type: lookup(TRANSFER_SOURCE_TYPE_HEADER)?,
            compression: lookup(TRANSFER_COMPRESSION_HEADER)?,
            symlink_mode: lookup(TRANSFER_SYMLINK_MODE_HEADER)?,
        })
    }
}

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
