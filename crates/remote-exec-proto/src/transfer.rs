use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::wire;

pub const DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_TRANSFER_MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default)]
pub struct TransferLimits {
    pub max_archive_bytes: u64,
    pub max_entry_bytes: u64,
}

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES,
            max_entry_bytes: DEFAULT_TRANSFER_MAX_ENTRY_BYTES,
        }
    }
}

impl TransferLimits {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_archive_bytes > 0,
            "transfer_limits.max_archive_bytes must be greater than zero"
        );
        anyhow::ensure!(
            self.max_entry_bytes > 0,
            "transfer_limits.max_entry_bytes must be greater than zero"
        );
        anyhow::ensure!(
            self.max_entry_bytes <= self.max_archive_bytes,
            "transfer_limits.max_entry_bytes must be less than or equal to transfer_limits.max_archive_bytes"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
    Multiple,
}

const TRANSFER_SOURCE_TYPE_WIRE_VALUES: &[(TransferSourceType, &str)] = &[
    (TransferSourceType::File, "file"),
    (TransferSourceType::Directory, "directory"),
    (TransferSourceType::Multiple, "multiple"),
];

impl TransferSourceType {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Multiple => "multiple",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        wire::from_wire_value(value, TRANSFER_SOURCE_TYPE_WIRE_VALUES)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwrite {
    Fail,
    #[default]
    Merge,
    Replace,
}

const TRANSFER_OVERWRITE_WIRE_VALUES: &[(TransferOverwrite, &str)] = &[
    (TransferOverwrite::Fail, "fail"),
    (TransferOverwrite::Merge, "merge"),
    (TransferOverwrite::Replace, "replace"),
];

impl TransferOverwrite {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Merge => "merge",
            Self::Replace => "replace",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        wire::from_wire_value(value, TRANSFER_OVERWRITE_WIRE_VALUES)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSymlinkMode {
    #[default]
    Preserve,
    Follow,
    Skip,
}

const TRANSFER_SYMLINK_MODE_WIRE_VALUES: &[(TransferSymlinkMode, &str)] = &[
    (TransferSymlinkMode::Preserve, "preserve"),
    (TransferSymlinkMode::Follow, "follow"),
    (TransferSymlinkMode::Skip, "skip"),
];

impl TransferSymlinkMode {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Follow => "follow",
            Self::Skip => "skip",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        wire::from_wire_value(value, TRANSFER_SYMLINK_MODE_WIRE_VALUES)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferCompression {
    #[default]
    None,
    Zstd,
}

const TRANSFER_COMPRESSION_WIRE_VALUES: &[(TransferCompression, &str)] = &[
    (TransferCompression::None, "none"),
    (TransferCompression::Zstd, "zstd"),
];

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
        wire::from_wire_value(value, TRANSFER_COMPRESSION_WIRE_VALUES)
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
pub struct TransferImportRequest {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferImportMetadata {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub symlink_mode: TransferSymlinkMode,
}

impl TransferImportRequest {
    pub fn metadata(&self) -> TransferImportMetadata {
        TransferImportMetadata::from(self)
    }
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
