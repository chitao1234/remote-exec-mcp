use std::io::Read;
use std::path::Path;

use remote_exec_proto::rpc::{
    TransferImportRequest, TransferImportResponse, TransferOverwrite, TransferSourceType,
    TransferSymlinkMode, TransferWarning,
};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};
use remote_exec_proto::transfer::TransferLimits;

use crate::error::TransferError;

use super::codec::{open_archive_reader, wrap_archive_reader};
use super::entry::{ensure_supported_archive_entry_type, normalize_archive_entry_path};
use super::summary::{is_transfer_summary_path, read_transfer_summary};
use super::{archive_error_to_transfer_error, host_path, host_policy, internal_transfer_error};

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
    limits: TransferLimits,
) -> Result<TransferImportResponse, TransferError> {
    let (destination, replaced) = prepare_import_destination(request, sandbox, windows_posix_root)
        .await
        .map_err(archive_error_to_transfer_error)?;
    let archive_path = archive_path.to_path_buf();
    let request = request.clone();

    tokio::task::spawn_blocking(move || {
        extract_archive(&archive_path, &destination, &request, replaced, limits)
    })
    .await
    .map_err(internal_transfer_error)?
    .map_err(archive_error_to_transfer_error)
}

pub async fn import_archive_from_async_reader<R>(
    reader: R,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
    limits: TransferLimits,
) -> Result<TransferImportResponse, TransferError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (destination, replaced) = prepare_import_destination(request, sandbox, windows_posix_root)
        .await
        .map_err(archive_error_to_transfer_error)?;
    let request = request.clone();
    let runtime = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || {
        let reader = tokio_util::io::SyncIoBridge::new_with_handle(reader, runtime);
        let reader = wrap_archive_reader(reader, &request.compression)?;
        extract_archive_from_reader(reader, &destination, &request, replaced, limits)
    })
    .await
    .map_err(internal_transfer_error)?
    .map_err(archive_error_to_transfer_error)
}

async fn prepare_import_destination(
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<(std::path::PathBuf, bool)> {
    let destination = host_path(&request.destination_path, windows_posix_root)?;
    authorize_path(host_policy(), sandbox, SandboxAccess::Write, &destination).map_err(|err| {
        crate::transfer::transfer_error_from_sandbox_error(
            "transfer destination path",
            &request.destination_path,
            err,
        )
    })?;

    let replaced = prepare_destination(&destination, request).await?;
    Ok((destination, replaced))
}

async fn prepare_destination(
    destination: &Path,
    request: &TransferImportRequest,
) -> anyhow::Result<bool> {
    if let Some(parent) = destination.parent() {
        if request.create_parent {
            tokio::fs::create_dir_all(parent).await?;
        } else {
            let parent_exists = tokio::fs::metadata(parent)
                .await
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            if !parent_exists {
                return Err(TransferError::parent_missing(format!(
                    "destination parent `{}` does not exist",
                    parent.display()
                ))
                .into());
            }
        }
    }

    match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) => match request.overwrite {
            TransferOverwrite::Fail => Err(TransferError::destination_exists(format!(
                "destination path `{}` already exists",
                destination.display()
            ))
            .into()),
            TransferOverwrite::Merge => {
                ensure_merge_destination_is_compatible(destination, &metadata, request)?;
                Ok(false)
            }
            TransferOverwrite::Replace => {
                if metadata.is_dir() {
                    tokio::fs::remove_dir_all(destination).await?;
                } else {
                    tokio::fs::remove_file(destination).await?;
                }
                Ok(true)
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn ensure_merge_destination_is_compatible(
    destination: &Path,
    metadata: &std::fs::Metadata,
    request: &TransferImportRequest,
) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() {
        return Err(TransferError::destination_unsupported(format!(
            "destination path contains unsupported symlink `{}`",
            destination.display()
        ))
        .into());
    }

    match request.source_type {
        TransferSourceType::File => {
            if metadata.is_dir() {
                return Err(TransferError::destination_unsupported(format!(
                    "destination path `{}` is a directory",
                    destination.display()
                ))
                .into());
            }
        }
        TransferSourceType::Directory | TransferSourceType::Multiple => {
            if !metadata.is_dir() {
                return Err(TransferError::destination_unsupported(format!(
                    "destination path `{}` is not a directory",
                    destination.display()
                ))
                .into());
            }
        }
    }

    Ok(())
}

fn extract_archive(
    archive_path: &Path,
    destination_path: &Path,
    request: &TransferImportRequest,
    replaced: bool,
    limits: TransferLimits,
) -> anyhow::Result<TransferImportResponse> {
    let reader = open_archive_reader(archive_path, &request.compression)?;
    extract_archive_from_reader(reader, destination_path, request, replaced, limits)
}

fn extract_archive_from_reader<R: Read>(
    reader: R,
    destination_path: &Path,
    request: &TransferImportRequest,
    replaced: bool,
    limits: TransferLimits,
) -> anyhow::Result<TransferImportResponse> {
    let mut summary = new_import_summary(request, replaced);

    let mut archive = tar::Archive::new(reader);

    match request.source_type {
        TransferSourceType::File => extract_single_file_archive(
            &mut archive,
            destination_path,
            &request.symlink_mode,
            &mut summary,
            limits,
        )?,
        TransferSourceType::Directory | TransferSourceType::Multiple => extract_tree_archive(
            &mut archive,
            destination_path,
            &request.symlink_mode,
            &mut summary,
            limits,
        )?,
    }

    Ok(summary)
}

fn new_import_summary(request: &TransferImportRequest, replaced: bool) -> TransferImportResponse {
    TransferImportResponse {
        source_type: request.source_type.clone(),
        bytes_copied: 0,
        files_copied: 0,
        directories_copied: matches!(
            request.source_type,
            TransferSourceType::Directory | TransferSourceType::Multiple
        ) as u64,
        replaced,
        warnings: Vec::new(),
    }
}

fn extract_single_file_archive<R: Read>(
    archive: &mut tar::Archive<R>,
    destination_path: &Path,
    symlink_mode: &TransferSymlinkMode,
    summary: &mut TransferImportResponse,
    limits: TransferLimits,
) -> anyhow::Result<()> {
    let mut entries = archive.entries()?;
    let mut entry = entries
        .next()
        .ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
    let entry_type = entry.header().entry_type();
    if !(entry_type.is_file() || entry_type.is_symlink()) {
        return Err(
            TransferError::source_unsupported("archive entry is not a regular file").into(),
        );
    }

    if entry_type.is_symlink() {
        write_archive_symlink(&mut entry, destination_path, symlink_mode, summary)?;
    } else {
        summary.bytes_copied = write_archive_file(&mut entry, destination_path, limits, 0)?;
        summary.files_copied = 1;
    }

    for entry in entries {
        let mut entry = entry?;
        let raw_rel = entry.path()?.to_path_buf();
        let rel = normalize_archive_entry_path(&raw_rel)?;
        if !is_transfer_summary_path(&rel) {
            return Err(
                TransferError::source_unsupported("file archive contains extra entries").into(),
            );
        }
        summary.warnings.extend(read_transfer_summary(&mut entry)?);
    }
    Ok(())
}

fn extract_tree_archive<R: Read>(
    archive: &mut tar::Archive<R>,
    destination_path: &Path,
    symlink_mode: &TransferSymlinkMode,
    summary: &mut TransferImportResponse,
    limits: TransferLimits,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(destination_path)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        extract_tree_archive_entry(&mut entry, destination_path, symlink_mode, summary, limits)?;
    }
    Ok(())
}

fn extract_tree_archive_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    destination_path: &Path,
    symlink_mode: &TransferSymlinkMode,
    summary: &mut TransferImportResponse,
    limits: TransferLimits,
) -> anyhow::Result<()> {
    let raw_rel = entry.path()?.to_path_buf();
    let rel = normalize_archive_entry_path(&raw_rel)?;
    if is_transfer_summary_path(&rel) {
        summary.warnings.extend(read_transfer_summary(entry)?);
        return Ok(());
    }
    if rel.as_os_str().is_empty() {
        return Ok(());
    }

    let out = destination_path.join(&rel);
    let entry_type = entry.header().entry_type();
    ensure_supported_archive_entry_type(entry_type, &raw_rel)?;
    ensure_no_existing_symlink_in_path(destination_path, &out)?;

    if entry_type.is_dir() {
        std::fs::create_dir_all(&out)?;
        summary.directories_copied += 1;
        return Ok(());
    }

    if entry_type.is_symlink() {
        write_archive_symlink(entry, &out, symlink_mode, summary)?;
        return Ok(());
    }

    summary.bytes_copied += write_archive_file(entry, &out, limits, summary.bytes_copied)?;
    summary.files_copied += 1;
    Ok(())
}

fn write_archive_symlink<R: Read>(
    entry: &mut tar::Entry<R>,
    path: &Path,
    symlink_mode: &TransferSymlinkMode,
    summary: &mut TransferImportResponse,
) -> anyhow::Result<()> {
    match symlink_import_action(symlink_mode, entry, path)? {
        SymlinkImportAction::Preserve(target) => {
            ensure_not_existing_symlink(path)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            remove_existing_file_before_symlink(path)?;
            create_symlink(&target, path)?;
            summary.files_copied += 1;
            Ok(())
        }
        SymlinkImportAction::Skip => {
            summary
                .warnings
                .push(TransferWarning::skipped_symlink(path.display()));
            Ok(())
        }
        SymlinkImportAction::Unsupported => Err(TransferError::source_unsupported(format!(
            "archive contains unsupported symlink `{}`",
            path.display()
        ))
        .into()),
    }
}

fn remove_existing_file_before_symlink(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err(TransferError::destination_unsupported(format!(
            "destination path `{}` is a directory",
            path.display()
        ))
        .into()),
        Ok(_) => {
            std::fs::remove_file(path)?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

enum SymlinkImportAction {
    Preserve(std::path::PathBuf),
    Skip,
    Unsupported,
}

fn symlink_import_action<R: Read>(
    symlink_mode: &TransferSymlinkMode,
    entry: &tar::Entry<R>,
    path: &Path,
) -> anyhow::Result<SymlinkImportAction> {
    match symlink_mode {
        TransferSymlinkMode::Preserve => {
            let target = entry.link_name()?.ok_or_else(|| {
                TransferError::source_unsupported(format!(
                    "archive symlink entry `{}` has no link target",
                    path.display()
                ))
            })?;
            Ok(SymlinkImportAction::Preserve(target.into_owned()))
        }
        TransferSymlinkMode::Skip => Ok(SymlinkImportAction::Skip),
        TransferSymlinkMode::Follow => Ok(SymlinkImportAction::Unsupported),
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, path: &Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(target, path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, path: &Path) -> anyhow::Result<()> {
    Err(TransferError::source_unsupported(format!(
        "archive contains unsupported symlink `{}`",
        path.display()
    ))
    .into())
}

fn write_archive_file<R: Read>(
    entry: &mut tar::Entry<R>,
    path: &Path,
    limits: TransferLimits,
    copied_so_far: u64,
) -> anyhow::Result<u64> {
    ensure_not_existing_symlink(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let entry_size = entry.header().size()?;
    let mode = entry.header().mode()?;
    ensure_entry_within_limits(entry_size, copied_so_far, limits)?;

    let mut output = std::fs::File::create(path)?;
    let copied = std::io::copy(&mut entry.take(entry_size), &mut output)?;
    if copied != entry_size {
        anyhow::bail!("truncated archive entry");
    }
    restore_executable_bits(path, mode)?;
    Ok(copied)
}

fn ensure_entry_within_limits(
    entry_size: u64,
    copied_so_far: u64,
    limits: TransferLimits,
) -> anyhow::Result<()> {
    if entry_size > limits.max_entry_bytes {
        return Err(TransferError::failed(format!(
            "archive entry size {entry_size} exceeds transfer entry limit {}",
            limits.max_entry_bytes
        ))
        .into());
    }
    if copied_so_far.saturating_add(entry_size) > limits.max_archive_bytes {
        return Err(TransferError::failed(format!(
            "archive byte count exceeds transfer archive limit {}",
            limits.max_archive_bytes
        ))
        .into());
    }
    Ok(())
}

fn ensure_no_existing_symlink_in_path(root: &Path, path: &Path) -> anyhow::Result<()> {
    ensure_not_existing_symlink(root)?;
    let Ok(relative) = path.strip_prefix(root) else {
        ensure_not_existing_symlink(path)?;
        return Ok(());
    };

    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        ensure_not_existing_symlink(&current)?;
    }

    Ok(())
}

fn ensure_not_existing_symlink(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(TransferError::destination_unsupported(format!(
                    "destination path contains unsupported symlink `{}`",
                    path.display()
                ))
                .into());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

#[cfg(unix)]
fn restore_executable_bits(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if mode & 0o111 != 0 {
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restore_executable_bits(_path: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}
