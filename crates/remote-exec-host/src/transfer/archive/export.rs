mod bundle;
mod prepare;
mod single;

use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use bytes::Bytes;
use remote_exec_proto::rpc::{TransferSourceType, TransferSymlinkMode};
use remote_exec_proto::transfer::TransferCompression;
use tokio::sync::mpsc;

use crate::error::TransferError;
use crate::sandbox::CompiledFilesystemSandbox;

use super::exclude_matcher::ExcludeMatcher;
use super::{
    BundledArchiveSource, ExportArchiveStreamItem, ExportPathResult, ExportedArchive,
    ExportedArchiveByteStream, ExportedArchiveStream, archive_error_to_transfer_error,
    internal_transfer_error,
};

const STREAM_BUFFER_SIZE: usize = 64 * 1024;
const EXPORT_STREAM_CHANNEL_DEPTH: usize = 16;

pub(super) struct PreparedExport {
    pub(super) source_path: PathBuf,
    pub(super) source_type: TransferSourceType,
    pub(super) exclude_matcher: ExcludeMatcher,
    pub(super) sandbox: Option<CompiledFilesystemSandbox>,
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
            .await?;
    let archive_path = archive_path.to_path_buf();
    let source_type = prepared.source_type.clone();

    let warnings =
        single::write_prepared_export_to_file(prepared, archive_path, compression, symlink_mode)
            .await?;

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
            .await?;
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

pub async fn export_path_to_byte_stream(
    path: &str,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<ExportedArchiveByteStream, TransferError> {
    let prepared =
        prepare::prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root)
            .await?;
    let source_type = prepared.source_type.clone();
    let stream_compression = compression.clone();
    let stream_symlink_mode = symlink_mode.clone();
    let (sender, receiver) = mpsc::channel(EXPORT_STREAM_CHANNEL_DEPTH);

    tokio::task::spawn_blocking(move || {
        let archive_bytes = Arc::new(AtomicU64::new(0));
        let writer = ChannelArchiveWriter {
            sender: sender.clone(),
            archive_bytes: Arc::clone(&archive_bytes),
        };
        let terminal = match single::write_prepared_export_to_writer_sync(
            prepared,
            writer,
            stream_compression,
            stream_symlink_mode,
        ) {
            Ok(_) => ExportArchiveStreamItem::Complete {
                archive_bytes: archive_bytes.load(Ordering::Relaxed),
            },
            Err(err) => ExportArchiveStreamItem::Error(err),
        };
        let _ = sender.blocking_send(terminal);
    });

    Ok(ExportedArchiveByteStream {
        source_type,
        compression,
        receiver,
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
            .map_err(archive_error_to_transfer_error)
    })
    .await
    .map_err(internal_transfer_error)?;
    result?;

    Ok(())
}

struct ChannelArchiveWriter {
    sender: mpsc::Sender<ExportArchiveStreamItem>,
    archive_bytes: Arc<AtomicU64>,
}

impl Write for ChannelArchiveWriter {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let total = buf.len();
        while !buf.is_empty() {
            let chunk_len = buf.len().min(STREAM_BUFFER_SIZE);
            let chunk = Bytes::copy_from_slice(&buf[..chunk_len]);
            self.sender
                .blocking_send(ExportArchiveStreamItem::Data(chunk))
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "transfer stream receiver closed")
                })?;
            self.archive_bytes
                .fetch_add(chunk_len as u64, Ordering::Relaxed);
            buf = &buf[chunk_len..];
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
