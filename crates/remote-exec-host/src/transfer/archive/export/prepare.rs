use std::path::{Path, PathBuf};

use remote_exec_proto::rpc::{TransferSourceType, TransferSymlinkMode};

use crate::error::TransferError;
use crate::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};

use super::super::exclude_matcher::ExcludeMatcher;
use super::super::{archive_error_to_transfer_error, host_path, internal_transfer_error};
use super::PreparedExport;

pub(super) async fn prepare_export_path(
    path: &str,
    symlink_mode: &TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<PreparedExport, TransferError> {
    let source_text = path.to_string();
    let source_path =
        host_path(&source_text, windows_posix_root).map_err(internal_transfer_error)?;
    authorize_path(sandbox, SandboxAccess::Read, &source_path).map_err(|err| {
        crate::transfer::transfer_error_from_sandbox_error(
            "transfer source path",
            &source_text,
            err,
        )
    })?;

    let metadata = tokio::fs::symlink_metadata(&source_path)
        .await
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                TransferError::source_missing(format!(
                    "transfer source path `{}` does not exist",
                    source_path.display()
                ))
            } else {
                TransferError::internal(err.to_string())
            }
        })?;
    let source_type = export_source_type_from_metadata(&source_path, &metadata, symlink_mode)?;
    let exclude_matcher =
        ExcludeMatcher::compile(exclude).map_err(archive_error_to_transfer_error)?;

    Ok(PreparedExport {
        source_path,
        source_type,
        exclude_matcher,
    })
}

fn export_source_type_from_metadata(
    path: &PathBuf,
    metadata: &std::fs::Metadata,
    symlink_mode: &TransferSymlinkMode,
) -> Result<TransferSourceType, TransferError> {
    if metadata.file_type().is_symlink() {
        match symlink_mode {
            TransferSymlinkMode::Preserve => return Ok(TransferSourceType::File),
            TransferSymlinkMode::Follow => {
                let target_metadata = std::fs::metadata(path).map_err(internal_transfer_error)?;
                if target_metadata.is_file() {
                    return Ok(TransferSourceType::File);
                }
                if target_metadata.is_dir() {
                    return Ok(TransferSourceType::Directory);
                }
                return Err(TransferError::source_unsupported(format!(
                    "transfer source symlink target `{}` is not a regular file or directory",
                    path.display()
                )));
            }
            TransferSymlinkMode::Skip => {
                return Err(TransferError::source_unsupported(format!(
                    "transfer source contains unsupported symlink `{}`",
                    path.display()
                )));
            }
        }
    }
    if metadata.file_type().is_file() {
        return Ok(TransferSourceType::File);
    }
    if metadata.file_type().is_dir() {
        return Ok(TransferSourceType::Directory);
    }

    Err(TransferError::source_unsupported(format!(
        "transfer source path `{}` is not a regular file or directory",
        path.display()
    )))
}
