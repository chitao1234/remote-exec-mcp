use std::path::{Path, PathBuf};

use crate::host_path;
use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::rpc::{TransferCompression, TransferSourceType};

mod codec;
mod entry;
mod export;
mod import;

pub use export::{bundle_archives_to_file, export_path_to_archive, export_path_to_file};
pub use import::import_archive_from_file;

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

pub struct ExportedArchive {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub temp_path: tempfile::TempPath,
}

pub struct BundledArchiveSource {
    pub source_path: String,
    pub source_policy: PathPolicy,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub archive_path: PathBuf,
}

pub(crate) fn host_policy() -> PathPolicy {
    host_path::host_path_policy()
}

pub(crate) fn host_path(raw: &str, windows_posix_root: Option<&Path>) -> anyhow::Result<PathBuf> {
    host_path::resolve_absolute_input_path(raw, windows_posix_root)
        .ok_or_else(|| anyhow::anyhow!("transfer path `{raw}` is not absolute"))
}
