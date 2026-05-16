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

wire::wire_value_mappings!(TransferSourceType {
    File => "file",
    Directory => "directory",
    Multiple => "multiple",
});

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwrite {
    Fail,
    #[default]
    Merge,
    Replace,
}

wire::wire_value_mappings!(TransferOverwrite {
    Fail => "fail",
    Merge => "merge",
    Replace => "replace",
});

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSymlinkMode {
    #[default]
    Preserve,
    Follow,
    Skip,
}

wire::wire_value_mappings!(TransferSymlinkMode {
    Preserve => "preserve",
    Follow => "follow",
    Skip => "skip",
});

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferCompression {
    #[default]
    None,
    Zstd,
}

wire::wire_value_mappings!(TransferCompression {
    None => "none",
    Zstd => "zstd",
});

impl TransferCompression {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
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
pub struct TransferImportSpec {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
}

pub type TransferImportRequest = TransferImportSpec;
pub type TransferImportMetadata = TransferImportSpec;

impl TransferImportSpec {
    pub fn metadata(&self) -> TransferImportMetadata {
        self.clone()
    }
}
