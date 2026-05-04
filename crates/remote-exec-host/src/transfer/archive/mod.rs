use std::path::{Path, PathBuf};

use crate::host_path;
use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::rpc::{TransferCompression, TransferSourceType, TransferWarning};

mod codec;
mod entry;
mod exclude_matcher;
mod export;
mod import;
mod summary;

pub use export::{
    bundle_archives_to_file, export_path_to_archive, export_path_to_file, export_path_to_stream,
};
pub use import::{import_archive_from_async_reader, import_archive_from_file};

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";
pub const TRANSFER_SUMMARY_ENTRY: &str = ".remote-exec-transfer-summary.json";

pub struct ExportedArchive {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub temp_path: tempfile::TempPath,
    pub warnings: Vec<TransferWarning>,
}

pub struct ExportedArchiveStream {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub reader: tokio::io::DuplexStream,
}

pub struct BundledArchiveSource {
    pub source_path: String,
    pub source_policy: PathPolicy,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub archive_path: PathBuf,
}

pub struct ExportPathResult {
    pub source_type: TransferSourceType,
    pub warnings: Vec<TransferWarning>,
}

pub(crate) fn host_policy() -> PathPolicy {
    host_path::host_path_policy()
}

pub(crate) fn host_path(raw: &str, windows_posix_root: Option<&Path>) -> anyhow::Result<PathBuf> {
    host_path::resolve_absolute_input_path(raw, windows_posix_root)
        .ok_or_else(|| anyhow::anyhow!("transfer path `{raw}` is not absolute"))
}
