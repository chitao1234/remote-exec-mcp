use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::host_path;
use remote_exec_proto::path::{PathPolicy, basename_for_policy, normalize_relative_path};
use remote_exec_proto::rpc::{
    TransferCompression, TransferImportRequest, TransferImportResponse, TransferOverwriteMode,
    TransferSourceType,
};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

pub struct ExportedArchive {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub temp_path: tempfile::TempPath,
}

pub struct BundledArchiveSource {
    pub source_path: String,
    pub source_policy: PathPolicy,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub archive_path: PathBuf,
}

fn host_policy() -> PathPolicy {
    host_path::host_path_policy()
}

fn host_path(raw: &str, windows_posix_root: Option<&Path>) -> anyhow::Result<std::path::PathBuf> {
    host_path::resolve_absolute_input_path(raw, windows_posix_root)
        .ok_or_else(|| anyhow::anyhow!("transfer path `{raw}` is not absolute"))
}

pub async fn export_path_to_archive(
    path: &str,
    compression: TransferCompression,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<ExportedArchive> {
    let temp = tempfile::NamedTempFile::new()?;
    let temp_path = temp.into_temp_path();
    let source_type = export_path_to_file(
        path,
        temp_path.as_ref(),
        compression.clone(),
        sandbox,
        windows_posix_root,
    )
    .await?;

    Ok(ExportedArchive {
        source_type,
        compression,
        temp_path,
    })
}

pub async fn export_path_to_file(
    path: &str,
    archive_path: &Path,
    compression: TransferCompression,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<TransferSourceType> {
    let source_text = path.to_string();
    anyhow::ensure!(
        host_path::is_input_path_absolute(&source_text, windows_posix_root),
        "transfer source path `{source_text}` is not absolute"
    );
    let path = host_path(&source_text, windows_posix_root)?;
    authorize_path(host_policy(), sandbox, SandboxAccess::Read, &path)?;

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

    let archive_path = archive_path.to_path_buf();
    let source_path = path.to_path_buf();
    let source_type_for_task = source_type.clone();

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let writer = open_archive_writer(&archive_path, &compression)?;
        let mut builder = tar::Builder::new(writer);

        match source_type_for_task {
            TransferSourceType::File => {
                builder.append_path_with_name(&source_path, SINGLE_FILE_ENTRY)?;
            }
            TransferSourceType::Directory => {
                builder.append_dir(".", &source_path)?;
                append_directory_entries(&mut builder, &source_path, &source_path)?;
            }
            TransferSourceType::Multiple => {
                anyhow::bail!("single-path export cannot produce a multi-source archive");
            }
        }

        finish_archive_builder(builder)
    })
    .await??;

    Ok(source_type)
}

pub async fn bundle_archives_to_file(
    sources: Vec<BundledArchiveSource>,
    archive_path: &Path,
    compression: TransferCompression,
) -> anyhow::Result<()> {
    let archive_path = archive_path.to_path_buf();

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let writer = open_archive_writer(&archive_path, &compression)?;
        let mut builder = tar::Builder::new(writer);
        for source in &sources {
            append_source_archive(&mut builder, source)?;
        }
        finish_archive_builder(builder)
    })
    .await??;

    Ok(())
}

fn append_directory_entries<W: Write>(
    builder: &mut tar::Builder<W>,
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

fn append_source_archive<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &BundledArchiveSource,
) -> anyhow::Result<()> {
    let root_name =
        basename_for_policy(source.source_policy, &source.source_path).ok_or_else(|| {
            anyhow::anyhow!(
                "transfer source path `{}` has no usable basename for multi-source transfer",
                source.source_path
            )
        })?;
    let reader = open_archive_reader(&source.archive_path, &source.compression)?;
    let mut archive = tar::Archive::new(reader);

    match source.source_type {
        TransferSourceType::File => {
            append_file_archive_to_bundle(builder, &mut archive, &root_name)
        }
        TransferSourceType::Directory => {
            append_directory_archive_to_bundle(builder, &mut archive, &root_name)
        }
        TransferSourceType::Multiple => {
            anyhow::bail!("multi-source archives cannot be nested");
        }
    }
}

fn normalize_archive_entry_path(raw_path: &Path) -> anyhow::Result<PathBuf> {
    normalize_relative_path(raw_path).ok_or_else(|| unsupported_archive_entry(raw_path))
}

fn ensure_supported_archive_entry_type(
    entry_type: tar::EntryType,
    raw_path: &Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        entry_type.is_dir() || entry_type.is_file(),
        "archive contains unsupported entry `{}`",
        raw_path.display()
    );
    Ok(())
}

fn unsupported_archive_entry(raw_path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "archive contains unsupported entry `{}`",
        raw_path.display()
    )
}

fn append_file_archive_to_bundle<W: Write, R: Read>(
    builder: &mut tar::Builder<W>,
    archive: &mut tar::Archive<R>,
    root_name: &str,
) -> anyhow::Result<()> {
    let mut entries = archive.entries()?;
    let mut entry = entries
        .next()
        .ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
    anyhow::ensure!(
        entry.header().entry_type().is_file(),
        "archive entry is not a regular file"
    );

    let raw_path = entry.path()?.to_path_buf();
    let rel = normalize_archive_entry_path(&raw_path)?;
    anyhow::ensure!(
        rel == Path::new(SINGLE_FILE_ENTRY),
        "file archive entry path must be `{SINGLE_FILE_ENTRY}`"
    );

    let mut header = entry.header().clone();
    builder.append_data(&mut header, root_name, &mut entry)?;
    anyhow::ensure!(
        entries.next().transpose()?.is_none(),
        "file archive contains extra entries"
    );
    Ok(())
}

fn append_directory_archive_to_bundle<W: Write, R: Read>(
    builder: &mut tar::Builder<W>,
    archive: &mut tar::Archive<R>,
    root_name: &str,
) -> anyhow::Result<()> {
    for entry in archive.entries()? {
        let mut entry = entry?;
        let raw_rel = entry.path()?.to_path_buf();
        let rel = normalize_archive_entry_path(&raw_rel)?;
        let entry_type = entry.header().entry_type();
        ensure_supported_archive_entry_type(entry_type, &raw_rel)?;

        if rel.as_os_str().is_empty() {
            anyhow::ensure!(entry_type.is_dir(), "archive file entry cannot target root");
            let mut header = entry.header().clone();
            builder.append_data(&mut header, root_name, std::io::empty())?;
            continue;
        }

        let out_rel = PathBuf::from(root_name).join(&rel);
        let mut header = entry.header().clone();
        if entry_type.is_dir() {
            builder.append_data(&mut header, &out_rel, std::io::empty())?;
            continue;
        }

        builder.append_data(&mut header, &out_rel, &mut entry)?;
    }

    Ok(())
}

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<TransferImportResponse> {
    anyhow::ensure!(
        host_path::is_input_path_absolute(&request.destination_path, windows_posix_root),
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
        directories_copied: matches!(
            request.source_type,
            TransferSourceType::Directory | TransferSourceType::Multiple
        ) as u64,
        replaced,
    };

    let reader = open_archive_reader(archive_path, &request.compression)?;
    let mut archive = tar::Archive::new(reader);

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
            let bytes_written = write_archive_file(&mut entry, destination_path)?;
            anyhow::ensure!(
                entries.next().transpose()?.is_none(),
                "file archive contains extra entries"
            );
            summary.bytes_copied = bytes_written;
            summary.files_copied = 1;
        }
        TransferSourceType::Directory | TransferSourceType::Multiple => {
            std::fs::create_dir_all(destination_path)?;
            for entry in archive.entries()? {
                let mut entry = entry?;
                let raw_rel = entry.path()?.to_path_buf();
                let rel = normalize_archive_entry_path(&raw_rel)?;
                if rel.as_os_str().is_empty() {
                    continue;
                }

                let out = destination_path.join(&rel);
                let entry_type = entry.header().entry_type();
                ensure_supported_archive_entry_type(entry_type, &raw_rel)?;

                if entry_type.is_dir() {
                    std::fs::create_dir_all(&out)?;
                    summary.directories_copied += 1;
                    continue;
                }

                summary.bytes_copied += write_archive_file(&mut entry, &out)?;
                summary.files_copied += 1;
            }
        }
    }

    Ok(summary)
}

fn write_archive_file<R: Read>(entry: &mut tar::Entry<R>, path: &Path) -> anyhow::Result<u64> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut bytes = Vec::new();
    std::io::Read::read_to_end(entry, &mut bytes)?;
    std::fs::write(path, &bytes)?;
    restore_executable_bits(path, entry.header().mode()?)?;
    Ok(bytes.len() as u64)
}

fn open_archive_writer(
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Write>> {
    let file = std::fs::File::create(archive_path)?;
    match compression {
        TransferCompression::None => Ok(Box::new(file)),
        TransferCompression::Zstd => {
            let encoder = zstd::stream::write::Encoder::new(file, 0)?;
            Ok(Box::new(encoder.auto_finish()))
        }
    }
}

fn open_archive_reader(
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Read>> {
    let file = std::fs::File::open(archive_path)?;
    match compression {
        TransferCompression::None => Ok(Box::new(file)),
        TransferCompression::Zstd => Ok(Box::new(zstd::stream::read::Decoder::new(file)?)),
    }
}

fn finish_archive_builder<W: Write>(mut builder: tar::Builder<W>) -> anyhow::Result<()> {
    builder.finish()?;
    drop(builder.into_inner()?);
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
