use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use remote_exec_proto::path::basename_for_policy;
use remote_exec_proto::rpc::{TransferCompression, TransferSourceType};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};

use super::codec::{open_archive_reader, with_archive_builder};
use super::entry::{ensure_supported_archive_entry_type, normalize_archive_entry_path};
use super::{BundledArchiveSource, ExportedArchive, SINGLE_FILE_ENTRY, host_path, host_policy};

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
        crate::host_path::is_input_path_absolute(&source_text, windows_posix_root),
        "transfer source path `{source_text}` is not absolute"
    );
    let path = host_path(&source_text, windows_posix_root)?;
    authorize_path(host_policy(), sandbox, SandboxAccess::Read, &path)?;

    let metadata = tokio::fs::symlink_metadata(&path).await?;
    let source_type = export_source_type_from_metadata(&path, &metadata)?;

    let archive_path = archive_path.to_path_buf();
    let source_path = path.to_path_buf();
    let source_type_for_task = source_type.clone();

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        with_archive_builder(&archive_path, &compression, |builder| {
            append_export_source(builder, &source_path, source_type_for_task)
        })
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
        with_archive_builder(&archive_path, &compression, |builder| {
            for source in &sources {
                append_source_archive(builder, source)?;
            }
            Ok(())
        })
    })
    .await??;

    Ok(())
}

fn export_source_type_from_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> anyhow::Result<TransferSourceType> {
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "transfer source contains unsupported symlink `{}`",
            path.display()
        );
    }
    if metadata.file_type().is_file() {
        return Ok(TransferSourceType::File);
    }
    if metadata.file_type().is_dir() {
        return Ok(TransferSourceType::Directory);
    }

    anyhow::bail!(
        "transfer source path `{}` is not a regular file or directory",
        path.display()
    );
}

fn append_export_source<W: Write>(
    builder: &mut tar::Builder<W>,
    source_path: &Path,
    source_type: TransferSourceType,
) -> anyhow::Result<()> {
    match source_type {
        TransferSourceType::File => {
            builder.append_path_with_name(source_path, SINGLE_FILE_ENTRY)?;
        }
        TransferSourceType::Directory => {
            builder.append_dir(".", source_path)?;
            append_directory_entries(builder, source_path, source_path)?;
        }
        TransferSourceType::Multiple => {
            anyhow::bail!("single-path export cannot produce a multi-source archive");
        }
    }

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
