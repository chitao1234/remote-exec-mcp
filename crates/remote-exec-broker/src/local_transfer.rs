use std::path::{Path, PathBuf};

use remote_exec_proto::path::{
    PathPolicy, is_absolute_for_policy, linux_path_policy, normalize_for_system,
    windows_path_policy,
};
use remote_exec_proto::rpc::{
    TransferImportRequest, TransferImportResponse, TransferOverwriteMode, TransferSourceType,
};

const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

fn host_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn host_path(raw: &str) -> PathBuf {
    PathBuf::from(normalize_for_system(host_policy(), raw))
}

pub async fn export_path_to_archive(
    path: &Path,
    archive_path: &Path,
) -> anyhow::Result<TransferSourceType> {
    let source_text = path.display().to_string();
    anyhow::ensure!(
        is_absolute_for_policy(host_policy(), &source_text),
        "transfer source path `{}` is not absolute",
        source_text
    );
    let path = host_path(&source_text);

    let metadata = tokio::fs::symlink_metadata(&path).await?;
    let source_type = if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "transfer source contains unsupported symlink `{}`",
            path.display()
        );
    } else if metadata.file_type().is_file() {
        TransferSourceType::File
    } else if metadata.file_type().is_dir() {
        TransferSourceType::Directory
    } else {
        anyhow::bail!(
            "transfer source path `{}` is not a regular file or directory",
            path.display()
        );
    };

    let source = path.to_path_buf();
    let destination = archive_path.to_path_buf();
    let source_type_for_task = source_type.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&destination)?;
        let mut builder = tar::Builder::new(file);
        match source_type_for_task {
            TransferSourceType::File => {
                builder.append_path_with_name(&source, SINGLE_FILE_ENTRY)?
            }
            TransferSourceType::Directory => {
                builder.append_dir(".", &source)?;
                append_directory_entries(&mut builder, &source, &source)?;
            }
        }
        builder.finish()?;
        Ok(())
    })
    .await??;

    Ok(source_type)
}

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    anyhow::ensure!(
        is_absolute_for_policy(host_policy(), &request.destination_path),
        "transfer destination path `{}` is not absolute",
        request.destination_path
    );
    let destination = host_path(&request.destination_path);
    anyhow::ensure!(
        destination.is_absolute(),
        "transfer destination path `{}` is not absolute",
        destination.display()
    );
    let replaced = prepare_destination(&destination, request).await?;
    let archive = archive_path.to_path_buf();
    let request = request.clone();
    tokio::task::spawn_blocking(move || extract_archive(&archive, &destination, &request, replaced))
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

fn append_directory_entries(
    builder: &mut tar::Builder<std::fs::File>,
    root: &Path,
    current: &Path,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "transfer source contains unsupported symlink `{}`",
            path.display()
        );
        if metadata.is_dir() {
            builder.append_dir(rel, &path)?;
            append_directory_entries(builder, root, &path)?;
        } else if metadata.is_file() {
            builder.append_path_with_name(&path, rel)?;
        } else {
            anyhow::bail!(
                "transfer source contains unsupported entry `{}`",
                path.display()
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
    let mut summary = TransferImportResponse {
        source_type: request.source_type.clone(),
        bytes_copied: 0,
        files_copied: 0,
        directories_copied: matches!(request.source_type, TransferSourceType::Directory) as u64,
        replaced,
    };

    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);

    match request.source_type {
        TransferSourceType::File => {
            let mut entries = archive.entries()?;
            let mut entry = entries
                .next()
                .ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
            anyhow::ensure!(
                entry.header().entry_type().is_file(),
                "archive entry is not a regular file"
            );
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;
            if let Some(parent) = destination_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(destination_path, &bytes)?;
            restore_executable_bits(destination_path, entry.header().mode()?)?;
            anyhow::ensure!(
                entries.next().transpose()?.is_none(),
                "file archive contains extra entries"
            );
            summary.bytes_copied = bytes.len() as u64;
            summary.files_copied = 1;
        }
        TransferSourceType::Directory => {
            std::fs::create_dir_all(destination_path)?;
            for entry in archive.entries()? {
                let mut entry = entry?;
                let rel = entry.path()?.to_path_buf();
                if rel == Path::new(".") {
                    continue;
                }

                let out = destination_path.join(&rel);
                let entry_type = entry.header().entry_type();
                anyhow::ensure!(
                    entry_type.is_dir() || entry_type.is_file(),
                    "archive contains unsupported entry `{}`",
                    rel.display()
                );
                if entry_type.is_dir() {
                    std::fs::create_dir_all(&out)?;
                    summary.directories_copied += 1;
                    continue;
                }

                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                std::fs::write(&out, &bytes)?;
                restore_executable_bits(&out, entry.header().mode()?)?;
                summary.bytes_copied += bytes.len() as u64;
                summary.files_copied += 1;
            }
        }
    }

    Ok(summary)
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
