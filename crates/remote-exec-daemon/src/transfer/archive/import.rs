use std::io::Read;
use std::path::Path;

use remote_exec_proto::rpc::{
    TransferImportRequest, TransferImportResponse, TransferOverwriteMode, TransferSourceType,
    TransferSymlinkMode, TransferWarning,
};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};

use super::codec::open_archive_reader;
use super::entry::{ensure_supported_archive_entry_type, normalize_archive_entry_path};
use super::{host_path, host_policy};

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<TransferImportResponse> {
    anyhow::ensure!(
        crate::host_path::is_input_path_absolute(&request.destination_path, windows_posix_root),
        "transfer destination path `{}` is not absolute",
        request.destination_path
    );
    let destination = host_path(&request.destination_path, windows_posix_root)?;
    authorize_path(host_policy(), sandbox, SandboxAccess::Write, &destination)?;

    let replaced = prepare_destination(&destination, request).await?;
    let archive_path = archive_path.to_path_buf();
    let request = request.clone();

    tokio::task::spawn_blocking(move || {
        extract_archive(&archive_path, &destination, &request, replaced)
    })
    .await?
}

async fn prepare_destination(
    destination: &Path,
    request: &TransferImportRequest,
) -> anyhow::Result<bool> {
    if let Some(parent) = destination.parent() {
        if request.create_parent {
            tokio::fs::create_dir_all(parent).await?;
        } else {
            anyhow::ensure!(
                tokio::fs::metadata(parent)
                    .await
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false),
                "destination parent `{}` does not exist",
                parent.display()
            );
        }
    }

    match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) => match request.overwrite {
            TransferOverwriteMode::Fail => {
                anyhow::bail!(
                    "destination path `{}` already exists",
                    destination.display()
                );
            }
            TransferOverwriteMode::Merge => {
                ensure_merge_destination_is_compatible(destination, &metadata, request)?;
                Ok(false)
            }
            TransferOverwriteMode::Replace => {
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
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "destination path contains unsupported symlink `{}`",
        destination.display()
    );

    match request.source_type {
        TransferSourceType::File => {
            anyhow::ensure!(
                !metadata.is_dir(),
                "destination path `{}` is a directory",
                destination.display()
            );
        }
        TransferSourceType::Directory | TransferSourceType::Multiple => {
            anyhow::ensure!(
                metadata.is_dir(),
                "destination path `{}` is not a directory",
                destination.display()
            );
        }
    }

    Ok(())
}

fn extract_archive(
    archive_path: &Path,
    destination_path: &Path,
    request: &TransferImportRequest,
    replaced: bool,
) -> anyhow::Result<TransferImportResponse> {
    let mut summary = new_import_summary(request, replaced);

    let reader = open_archive_reader(archive_path, &request.compression)?;
    let mut archive = tar::Archive::new(reader);

    match request.source_type {
        TransferSourceType::File => extract_single_file_archive(
            &mut archive,
            destination_path,
            &request.symlink_mode,
            &mut summary,
        )?,
        TransferSourceType::Directory | TransferSourceType::Multiple => extract_tree_archive(
            &mut archive,
            destination_path,
            &request.symlink_mode,
            &mut summary,
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
) -> anyhow::Result<()> {
    let mut entries = archive.entries()?;
    let mut entry = entries
        .next()
        .ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
    let entry_type = entry.header().entry_type();
    anyhow::ensure!(
        entry_type.is_file() || entry_type.is_symlink(),
        "archive entry is not a regular file"
    );

    if entry_type.is_symlink() {
        write_archive_symlink(&mut entry, destination_path, symlink_mode, summary)?;
    } else {
        summary.bytes_copied = write_archive_file(&mut entry, destination_path)?;
        summary.files_copied = 1;
    }

    anyhow::ensure!(
        entries.next().transpose()?.is_none(),
        "file archive contains extra entries"
    );
    Ok(())
}

fn extract_tree_archive<R: Read>(
    archive: &mut tar::Archive<R>,
    destination_path: &Path,
    symlink_mode: &TransferSymlinkMode,
    summary: &mut TransferImportResponse,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(destination_path)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        extract_tree_archive_entry(&mut entry, destination_path, symlink_mode, summary)?;
    }
    Ok(())
}

fn extract_tree_archive_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    destination_path: &Path,
    symlink_mode: &TransferSymlinkMode,
    summary: &mut TransferImportResponse,
) -> anyhow::Result<()> {
    let raw_rel = entry.path()?.to_path_buf();
    let rel = normalize_archive_entry_path(&raw_rel)?;
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

    summary.bytes_copied += write_archive_file(entry, &out)?;
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
        SymlinkImportAction::Reject => {
            anyhow::bail!("archive contains unsupported symlink `{}`", path.display())
        }
    }
}

fn remove_existing_file_before_symlink(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            anyhow::bail!("destination path `{}` is a directory", path.display())
        }
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
    Reject,
}

fn symlink_import_action<R: Read>(
    symlink_mode: &TransferSymlinkMode,
    entry: &tar::Entry<R>,
    path: &Path,
) -> anyhow::Result<SymlinkImportAction> {
    match symlink_mode {
        TransferSymlinkMode::Preserve => {
            let target = entry.link_name()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "archive symlink entry `{}` has no link target",
                    path.display()
                )
            })?;
            Ok(SymlinkImportAction::Preserve(target.into_owned()))
        }
        TransferSymlinkMode::Skip => Ok(SymlinkImportAction::Skip),
        TransferSymlinkMode::Follow | TransferSymlinkMode::Reject => {
            Ok(SymlinkImportAction::Reject)
        }
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, path: &Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(target, path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, path: &Path) -> anyhow::Result<()> {
    anyhow::bail!("archive contains unsupported symlink `{}`", path.display())
}

fn write_archive_file<R: Read>(entry: &mut tar::Entry<R>, path: &Path) -> anyhow::Result<u64> {
    ensure_not_existing_symlink(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut bytes = Vec::new();
    std::io::Read::read_to_end(entry, &mut bytes)?;
    std::fs::write(path, &bytes)?;
    restore_executable_bits(path, entry.header().mode()?)?;
    Ok(bytes.len() as u64)
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
        Ok(metadata) => anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "destination path contains unsupported symlink `{}`",
            path.display()
        ),
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
