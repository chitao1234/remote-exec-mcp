use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
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

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwrite {
    Fail,
    #[default]
    Merge,
    Replace,
}

impl TransferOverwrite {
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

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferExportRequest {
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "crate::rpc::TransferCompression::is_none"
    )]
    pub compression: crate::rpc::TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExportMetadata {
    pub source_type: TransferSourceType,
    pub compression: crate::rpc::TransferCompression,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportRequest {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: crate::rpc::TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferImportMetadata {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: crate::rpc::TransferCompression,
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
