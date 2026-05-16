use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rpc::TransferWarning;
use crate::transfer::{TransferOverwrite, TransferSourceType, TransferSymlinkMode};

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferDestinationMode {
    #[default]
    Auto,
    Exact,
    IntoDirectory,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub overwrite: TransferOverwrite,
    #[serde(default)]
    pub destination_mode: TransferDestinationMode,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    pub create_parent: bool,
}

impl TransferFilesInput {
    pub fn resolved_sources(&self) -> anyhow::Result<Vec<TransferEndpoint>> {
        match (&self.source, self.sources.is_empty()) {
            (Some(_), false) => anyhow::bail!("provide either `source` or `sources`, not both"),
            (Some(source), true) => Ok(vec![source.clone()]),
            (None, false) => Ok(self.sources.clone()),
            (None, true) => anyhow::bail!("`sources` must contain at least one entry"),
        }
    }
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
