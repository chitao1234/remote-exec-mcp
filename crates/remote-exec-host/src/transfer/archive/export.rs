mod bundle;
mod prepare;
mod single;

use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use bytes::Bytes;
use remote_exec_proto::transfer::TransferCompression;
use remote_exec_proto::transfer::{TransferSourceType, TransferSymlinkMode};
use tokio::sync::mpsc;

use crate::error::TransferError;
use crate::sandbox::CompiledFilesystemSandbox;

use super::exclude_matcher::ExcludeMatcher;
use super::{
    BundledArchiveSource, ExportArchiveStreamItem, ExportPathResult, ExportedArchive,
    ExportedArchiveByteStream, archive_error_to_transfer_error, internal_transfer_error,
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
        compression,
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
    let source_type = prepared.source_type;

    let warnings =
        single::write_prepared_export_to_file(prepared, archive_path, compression, symlink_mode)
            .await?;

    Ok(ExportPathResult {
        source_type,
        warnings,
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
    let source_type = prepared.source_type;
    let stream_compression = compression;
    let stream_symlink_mode = symlink_mode;
    let (sender, receiver) = mpsc::channel(EXPORT_STREAM_CHANNEL_DEPTH);

    tokio::task::spawn_blocking(move || {
        let archive_bytes = Arc::new(AtomicU64::new(0));
        let writer = ChannelArchiveWriter {
            state: Arc::new(Mutex::new(ChannelArchiveWriterState {
                sender: sender.clone(),
                archive_bytes: Arc::clone(&archive_bytes),
                pending: Vec::with_capacity(STREAM_BUFFER_SIZE),
            })),
        };
        let terminal = match single::write_prepared_export_to_writer_sync(
            prepared,
            writer.clone(),
            stream_compression,
            stream_symlink_mode,
        )
        .and_then(|_| writer.finish())
        {
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

#[derive(Clone)]
struct ChannelArchiveWriter {
    state: Arc<Mutex<ChannelArchiveWriterState>>,
}

struct ChannelArchiveWriterState {
    sender: mpsc::Sender<ExportArchiveStreamItem>,
    archive_bytes: Arc<AtomicU64>,
    pending: Vec<u8>,
}

impl ChannelArchiveWriter {
    fn finish(&self) -> Result<(), TransferError> {
        self.flush_pending()
            .map_err(|err| TransferError::internal(err.to_string()))
    }

    fn flush_pending(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("transfer stream writer lock poisoned"))?;
        if state.pending.is_empty() {
            return Ok(());
        }

        let bytes = Bytes::from(std::mem::take(&mut state.pending));
        send_archive_chunk(&mut state, bytes)
    }
}

impl Write for ChannelArchiveWriter {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let total = buf.len();
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("transfer stream writer lock poisoned"))?;

        if !state.pending.is_empty() {
            let needed = STREAM_BUFFER_SIZE - state.pending.len();
            let count = needed.min(buf.len());
            state.pending.extend_from_slice(&buf[..count]);
            buf = &buf[count..];
            if state.pending.len() == STREAM_BUFFER_SIZE {
                let bytes = Bytes::from(std::mem::take(&mut state.pending));
                send_archive_chunk(&mut state, bytes)?;
            }
        }

        while buf.len() >= STREAM_BUFFER_SIZE {
            let (chunk, remainder) = buf.split_at(STREAM_BUFFER_SIZE);
            send_archive_chunk(&mut state, Bytes::copy_from_slice(chunk))?;
            buf = remainder;
        }

        if !buf.is_empty() {
            state.pending.extend_from_slice(buf);
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_pending()
    }
}

fn send_archive_chunk(state: &mut ChannelArchiveWriterState, bytes: Bytes) -> io::Result<()> {
    let chunk_len = bytes.len();
    state
        .sender
        .blocking_send(ExportArchiveStreamItem::Data(bytes))
        .map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "transfer stream receiver closed")
        })?;
    state
        .archive_bytes
        .fetch_add(chunk_len as u64, Ordering::Relaxed);
    Ok(())
}
