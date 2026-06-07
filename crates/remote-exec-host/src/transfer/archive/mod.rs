use std::path::PathBuf;

use crate::host_path;
use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::transfer::TransferCompression;
use remote_exec_proto::{rpc::TransferWarning, transfer::TransferSourceType};

mod codec;
mod entry;
mod exclude_matcher;
mod export;
mod import;
mod summary;

pub use export::{
    bundle_archives_to_file, export_path_to_archive, export_path_to_byte_stream,
    export_path_to_file,
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

pub enum ExportArchiveStreamItem {
    Data(bytes::Bytes),
    Complete { archive_bytes: u64 },
    Error(crate::error::TransferError),
}

pub struct ExportedArchiveByteStream {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub receiver: tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>,
}

pub struct BundledArchiveSource {
    pub source_path: PathBuf,
    pub source_policy: PathPolicy,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub archive_path: PathBuf,
}

pub struct ExportPathResult {
    pub source_type: TransferSourceType,
    pub warnings: Vec<TransferWarning>,
}

pub(crate) fn host_path(
    raw: &str,
    windows_posix_root: Option<&std::path::Path>,
) -> anyhow::Result<host_path::ResolvedHostPath> {
    Ok(host_path::resolve_path_text_for_operation(
        raw,
        windows_posix_root,
    ))
}

pub(crate) fn archive_error_to_transfer_error(err: anyhow::Error) -> crate::error::TransferError {
    match err.downcast::<crate::error::TransferError>() {
        Ok(err) => err,
        Err(err) => internal_transfer_error(err),
    }
}

pub(crate) fn internal_transfer_error(err: impl std::fmt::Display) -> crate::error::TransferError {
    crate::error::TransferError::internal(err.to_string())
}
