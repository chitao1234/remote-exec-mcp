use std::path::Path;

use futures_util::StreamExt;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_host::transfer::archive::{ExportArchiveStreamItem, ExportedArchiveByteStream};
use remote_exec_proto::rpc::{
    TransferImportRequest, TransferImportResponse, TransferPathInfoResponse,
};
use remote_exec_proto::transfer::TransferLimits;

use crate::daemon_client::DaemonClientError;

pub async fn export_path_to_byte_stream(
    path: &str,
    request: &remote_exec_proto::rpc::TransferExportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<ExportedArchiveByteStream> {
    remote_exec_host::transfer::archive::export_path_to_byte_stream(
        path,
        request.compression,
        request.symlink_mode,
        &request.exclude,
        sandbox,
        windows_posix_root,
    )
    .await
    .map_err(Into::into)
}

pub fn export_byte_stream_reader(
    receiver: tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>,
) -> impl tokio::io::AsyncRead + Send + Unpin + 'static {
    enum ExportReadState {
        Items(tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>),
        Done,
    }

    enum ExportReadItem {
        Data(bytes::Bytes),
        Complete,
        Error(remote_exec_host::TransferError),
        MissingTerminal,
    }

    let stream = futures_util::stream::unfold(ExportReadState::Items(receiver), |state| async {
        match state {
            ExportReadState::Items(mut receiver) => {
                let item = receiver
                    .recv()
                    .await
                    .map(|item| match item {
                        ExportArchiveStreamItem::Data(bytes) => ExportReadItem::Data(bytes),
                        ExportArchiveStreamItem::Complete { .. } => ExportReadItem::Complete,
                        ExportArchiveStreamItem::Error(err) => ExportReadItem::Error(err),
                    })
                    .unwrap_or(ExportReadItem::MissingTerminal);
                let next = if matches!(
                    item,
                    ExportReadItem::Complete
                        | ExportReadItem::Error(_)
                        | ExportReadItem::MissingTerminal
                ) {
                    ExportReadState::Done
                } else {
                    ExportReadState::Items(receiver)
                };
                let result = match item {
                    ExportReadItem::Data(bytes) => Ok(bytes),
                    ExportReadItem::Complete => return None,
                    ExportReadItem::Error(err) => Err(std::io::Error::other(err)),
                    ExportReadItem::MissingTerminal => Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "transfer export stream ended before terminal state",
                    )),
                };
                Some((result, next))
            }
            ExportReadState::Done => None,
        }
    });
    tokio_util::io::StreamReader::new(stream.boxed())
}

pub async fn import_archive_from_async_reader<R>(
    reader: R,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
    limits: TransferLimits,
) -> anyhow::Result<TransferImportResponse>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    remote_exec_host::transfer::archive::import_archive_from_async_reader(
        reader,
        request,
        sandbox,
        windows_posix_root,
        limits,
    )
    .await
    .map_err(Into::into)
}

pub fn path_info(
    path: &str,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<TransferPathInfoResponse, DaemonClientError> {
    remote_exec_host::transfer::path_info_for_path(path, sandbox, windows_posix_root)
        .map_err(crate::local::backend::map_local_transfer_error)
}
