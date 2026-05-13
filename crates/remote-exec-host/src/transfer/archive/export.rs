mod bundle;
mod prepare;
mod single;

use std::path::{Path, PathBuf};

use remote_exec_proto::rpc::{TransferSourceType, TransferSymlinkMode};
use remote_exec_proto::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::transfer::TransferCompression;

use crate::error::TransferError;

use super::exclude_matcher::ExcludeMatcher;
use super::{
    BundledArchiveSource, ExportPathResult, ExportedArchive, ExportedArchiveStream,
    archive_error_to_transfer_error, internal_transfer_error,
};

const STREAM_BUFFER_SIZE: usize = 64 * 1024;

pub(super) struct PreparedExport {
    pub(super) source_path: PathBuf,
    pub(super) source_type: TransferSourceType,
    pub(super) exclude_matcher: ExcludeMatcher,
}

pub async fn export_path_to_archive(
    path: &str,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<ExportedArchive, TransferError> {
    let temp = tempfile::NamedTempFile::new().map_err(internal_transfer_error)?;
    let temp_path = temp.into_temp_path();
    let exported = export_path_to_file(
        path,
        temp_path.as_ref(),
        compression.clone(),
        symlink_mode,
        exclude,
        sandbox,
        windows_posix_root,
    )
    .await?;

    Ok(ExportedArchive {
        source_type: exported.source_type,
        compression,
        temp_path,
        warnings: exported.warnings,
    })
}

pub async fn export_path_to_file(
    path: &str,
    archive_path: &Path,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<ExportPathResult, TransferError> {
    let prepared =
        prepare::prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root)
            .await
            .map_err(archive_error_to_transfer_error)?;
    let archive_path = archive_path.to_path_buf();
    let source_type = prepared.source_type.clone();

    let warnings =
        single::write_prepared_export_to_file(prepared, archive_path, compression, symlink_mode)
            .await
            .map_err(archive_error_to_transfer_error)?;

    Ok(ExportPathResult {
        source_type,
        warnings,
    })
}

pub async fn export_path_to_stream(
    path: &str,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<ExportedArchiveStream, TransferError> {
    let prepared =
        prepare::prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root)
            .await
            .map_err(archive_error_to_transfer_error)?;
    let source_type = prepared.source_type.clone();
    let (reader, writer) = tokio::io::duplex(STREAM_BUFFER_SIZE);
    let task_compression = compression.clone();
    tokio::spawn(async move {
        let writer = tokio_util::io::SyncIoBridge::new(writer);
        if let Err(err) = single::write_prepared_export_to_writer(
            prepared,
            writer,
            task_compression,
            symlink_mode,
        )
        .await
        {
            tracing::debug!(error = %err, "streamed transfer export stopped");
        }
    });

    Ok(ExportedArchiveStream {
        source_type,
        compression,
        reader,
    })
}

pub async fn bundle_archives_to_file(
    sources: Vec<BundledArchiveSource>,
    archive_path: &Path,
    compression: TransferCompression,
) -> Result<(), TransferError> {
    let archive_path = archive_path.to_path_buf();

    let result = tokio::task::spawn_blocking(move || {
        bundle::bundle_archives_to_file(&sources, &archive_path, &compression)
    })
    .await
    .map_err(internal_transfer_error)?;
    result.map_err(archive_error_to_transfer_error)?;

    Ok(())
}
