use std::path::{Path, PathBuf};

use remote_exec_proto::rpc::{TransferSourceType, TransferSymlinkMode};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};

use crate::error::TransferError;

use super::super::exclude_matcher::ExcludeMatcher;
use super::super::{host_path, host_policy};
use super::PreparedExport;

pub(super) async fn prepare_export_path(
    path: &str,
    symlink_mode: &TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<PreparedExport> {
    let source_text = path.to_string();
    let source_path = host_path(&source_text, windows_posix_root)?;
    authorize_path(host_policy(), sandbox, SandboxAccess::Read, &source_path).map_err(|err| {
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
    let exclude_matcher = ExcludeMatcher::compile(exclude)?;

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
) -> anyhow::Result<TransferSourceType> {
    if metadata.file_type().is_symlink() {
        match symlink_mode {
            TransferSymlinkMode::Preserve => return Ok(TransferSourceType::File),
            TransferSymlinkMode::Follow => {
                let target_metadata = std::fs::metadata(path)?;
                if target_metadata.is_file() {
                    return Ok(TransferSourceType::File);
                }
                if target_metadata.is_dir() {
                    return Ok(TransferSourceType::Directory);
                }
                return Err(TransferError::source_unsupported(format!(
                    "transfer source symlink target `{}` is not a regular file or directory",
                    path.display()
                ))
                .into());
            }
            TransferSymlinkMode::Skip => {
                return Err(TransferError::source_unsupported(format!(
                    "transfer source contains unsupported symlink `{}`",
                    path.display()
                ))
                .into());
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
    ))
    .into())
}
