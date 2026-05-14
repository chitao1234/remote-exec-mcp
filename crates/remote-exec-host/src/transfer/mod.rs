pub mod archive;

use std::sync::Arc;

use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};
use remote_exec_proto::transfer::TransferCompression;

use crate::{
    AppState,
    error::TransferError,
    sandbox::{SandboxAccess, SandboxError, authorize_path},
};

pub fn path_info_for_request(
    state: &AppState,
    req: &TransferPathInfoRequest,
) -> Result<TransferPathInfoResponse, TransferError> {
    let path = archive::host_path(&req.path, state.config.windows_posix_root.as_deref())
        .map_err(|err| TransferError::internal(err.to_string()))?;
    authorize_path(state.sandbox.as_ref(), SandboxAccess::Write, &path).map_err(|err| {
        transfer_error_from_sandbox_error("transfer endpoint path", &req.path, err)
    })?;

    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(TransferError::destination_unsupported(format!(
                    "destination path contains unsupported symlink `{}`",
                    path.display()
                )));
            }
            Ok(TransferPathInfoResponse {
                exists: true,
                is_directory: metadata.is_dir(),
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TransferPathInfoResponse {
            exists: false,
            is_directory: false,
        }),
        Err(err) => Err(TransferError::internal(err.to_string())),
    }
}

pub async fn export_path_local(
    state: Arc<AppState>,
    req: TransferExportRequest,
) -> Result<archive::ExportedArchiveStream, TransferError> {
    ensure_transfer_compression_supported(state.as_ref(), &req.compression)?;
    archive::export_path_to_stream(
        &req.path,
        req.compression,
        req.symlink_mode,
        &req.exclude,
        state.sandbox.as_ref(),
        state.config.windows_posix_root.as_deref(),
    )
    .await
}

pub async fn import_archive_local(
    state: Arc<AppState>,
    request: TransferImportRequest,
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> Result<TransferImportResponse, TransferError> {
    ensure_transfer_compression_supported(state.as_ref(), &request.compression)?;
    archive::import_archive_from_async_reader(
        reader,
        &request,
        state.sandbox.as_ref(),
        state.config.windows_posix_root.as_deref(),
        state.config.transfer_limits,
    )
    .await
}

fn ensure_transfer_compression_supported(
    state: &AppState,
    compression: &TransferCompression,
) -> Result<(), TransferError> {
    if matches!(compression, TransferCompression::Zstd) && !state.supports_transfer_compression {
        return Err(TransferError::compression_unsupported(
            "this daemon does not support transfer compression",
        ));
    }
    Ok(())
}

pub(crate) fn transfer_error_from_sandbox_error(
    label: &str,
    raw_path: &str,
    err: SandboxError,
) -> TransferError {
    match err {
        SandboxError::NotAbsolute { .. } => {
            TransferError::path_not_absolute(format!("{label} `{raw_path}` is not absolute"))
        }
        err => TransferError::sandbox_denied(err.to_string()),
    }
}
