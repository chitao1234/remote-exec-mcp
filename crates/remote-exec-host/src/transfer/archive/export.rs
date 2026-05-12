use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use remote_exec_proto::path::basename_for_policy;
use remote_exec_proto::rpc::{TransferSourceType, TransferSymlinkMode, TransferWarning};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};
use remote_exec_proto::transfer::TransferCompression;

use crate::error::TransferError;

use super::codec::{open_archive_reader, with_archive_builder, with_archive_writer};
use super::entry::{ensure_supported_archive_entry_type, normalize_archive_entry_path};
use super::exclude_matcher::{ExcludeMatcher, normalize_relative_path};
use super::summary::{append_transfer_summary, is_transfer_summary_path, read_transfer_summary};
use super::{
    BundledArchiveSource, ExportPathResult, ExportedArchive, ExportedArchiveStream,
    SINGLE_FILE_ENTRY, archive_error_to_transfer_error, host_path, host_policy,
    internal_transfer_error,
};

const STREAM_BUFFER_SIZE: usize = 64 * 1024;

struct PreparedExport {
    source_path: PathBuf,
    source_type: TransferSourceType,
    exclude_matcher: ExcludeMatcher,
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
        compression.clone(),
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
    let prepared = prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root)
        .await
        .map_err(archive_error_to_transfer_error)?;
    let archive_path = archive_path.to_path_buf();
    let source_type = prepared.source_type.clone();

    let warnings = write_prepared_export_to_file(prepared, archive_path, compression, symlink_mode)
        .await
        .map_err(archive_error_to_transfer_error)?;

    Ok(ExportPathResult {
        source_type,
        warnings,
    })
}

pub async fn export_path_to_stream(
    path: &str,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> Result<ExportedArchiveStream, TransferError> {
    let prepared = prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root)
        .await
        .map_err(archive_error_to_transfer_error)?;
    let source_type = prepared.source_type.clone();
    let (reader, writer) = tokio::io::duplex(STREAM_BUFFER_SIZE);
    let task_compression = compression.clone();
    tokio::spawn(async move {
        let writer = tokio_util::io::SyncIoBridge::new(writer);
        if let Err(err) =
            write_prepared_export_to_writer(prepared, writer, task_compression, symlink_mode).await
        {
            tracing::debug!(error = %err, "streamed transfer export stopped");
        }
    });

    Ok(ExportedArchiveStream {
        source_type,
        compression,
        reader,
    })
}

async fn prepare_export_path(
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

async fn write_prepared_export_to_file(
    prepared: PreparedExport,
    archive_path: PathBuf,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
) -> anyhow::Result<Vec<TransferWarning>> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<TransferWarning>> {
        let mut warnings = Vec::new();
        with_archive_builder(&archive_path, &compression, |builder| {
            warnings = append_export_source(
                builder,
                &prepared.source_path,
                prepared.source_type,
                &prepared.exclude_matcher,
                &symlink_mode,
            )?;
            append_transfer_summary(builder, &warnings)?;
            Ok(())
        })?;
        Ok(warnings)
    })
    .await?
}

async fn write_prepared_export_to_writer<W>(
    prepared: PreparedExport,
    writer: W,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
) -> anyhow::Result<Vec<TransferWarning>>
where
    W: Write + Send + 'static,
{
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<TransferWarning>> {
        let mut warnings = Vec::new();
        with_archive_writer(writer, &compression, |builder| {
            warnings = append_export_source(
                builder,
                &prepared.source_path,
                prepared.source_type,
                &prepared.exclude_matcher,
                &symlink_mode,
            )?;
            append_transfer_summary(builder, &warnings)?;
            Ok(())
        })?;
        Ok(warnings)
    })
    .await?
}

pub async fn bundle_archives_to_file(
    sources: Vec<BundledArchiveSource>,
    archive_path: &Path,
    compression: TransferCompression,
) -> Result<(), TransferError> {
    let archive_path = archive_path.to_path_buf();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        with_archive_builder(&archive_path, &compression, |builder| {
            let mut warnings = Vec::new();
            for source in &sources {
                warnings.extend(append_source_archive(builder, source)?);
            }
            append_transfer_summary(builder, &warnings)?;
            Ok(())
        })
    })
    .await
    .map_err(internal_transfer_error)?;
    result.map_err(archive_error_to_transfer_error)?;

    Ok(())
}

fn export_source_type_from_metadata(
    path: &Path,
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

fn append_export_source<W: Write>(
    builder: &mut tar::Builder<W>,
    source_path: &Path,
    source_type: TransferSourceType,
    exclude_matcher: &ExcludeMatcher,
    symlink_mode: &TransferSymlinkMode,
) -> anyhow::Result<Vec<TransferWarning>> {
    let mut warnings = Vec::new();
    match source_type {
        TransferSourceType::File => {
            append_file_or_symlink_entry(
                builder,
                source_path,
                Path::new(SINGLE_FILE_ENTRY),
                symlink_mode,
            )?;
        }
        TransferSourceType::Directory => {
            builder.append_dir(".", source_path)?;
            append_directory_entries(
                builder,
                source_path,
                source_path,
                exclude_matcher,
                symlink_mode,
                &mut warnings,
            )?;
        }
        TransferSourceType::Multiple => {
            anyhow::bail!("single-path export cannot produce a multi-source archive");
        }
    }

    Ok(warnings)
}

fn append_directory_entries<W: Write>(
    builder: &mut tar::Builder<W>,
    root: &Path,
    current: &Path,
    exclude_matcher: &ExcludeMatcher,
    symlink_mode: &TransferSymlinkMode,
    warnings: &mut Vec<TransferWarning>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let rel_text = normalize_relative_path(rel);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            if exclude_matcher.is_excluded_directory(&rel_text) {
                continue;
            }
        } else if exclude_matcher.is_excluded_path(&rel_text) {
            continue;
        }
        if metadata.file_type().is_symlink() {
            match symlink_mode {
                TransferSymlinkMode::Preserve => {
                    append_symlink_entry(builder, &path, rel)?;
                }
                TransferSymlinkMode::Follow => {
                    let target_metadata = std::fs::metadata(&path)?;
                    if target_metadata.is_dir() {
                        builder.append_dir(rel, &path)?;
                        append_directory_entries(
                            builder,
                            root,
                            &path,
                            exclude_matcher,
                            symlink_mode,
                            warnings,
                        )?;
                    } else if target_metadata.is_file() {
                        builder.append_path_with_name(&path, rel)?;
                    } else {
                        handle_unsupported_entry(&path, warnings);
                    }
                }
                TransferSymlinkMode::Skip => {
                    warnings.push(TransferWarning::skipped_symlink(path.display()));
                }
            }
            continue;
        }
        if metadata.is_dir() {
            builder.append_dir(rel, &path)?;
            append_directory_entries(
                builder,
                root,
                &path,
                exclude_matcher,
                symlink_mode,
                warnings,
            )?;
        } else if metadata.is_file() {
            builder.append_path_with_name(&path, rel)?;
        } else {
            handle_unsupported_entry(&path, warnings);
        }
    }

    Ok(())
}

fn append_file_or_symlink_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    source_path: &Path,
    archive_path: &Path,
    symlink_mode: &TransferSymlinkMode,
) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(source_path)?;
    if metadata.file_type().is_symlink() {
        match symlink_mode {
            TransferSymlinkMode::Preserve => {
                append_symlink_entry(builder, source_path, archive_path)?;
            }
            TransferSymlinkMode::Follow => {
                builder.append_path_with_name(source_path, archive_path)?
            }
            TransferSymlinkMode::Skip => {
                return Err(TransferError::source_unsupported(format!(
                    "transfer source contains unsupported symlink `{}`",
                    source_path.display()
                ))
                .into());
            }
        }
    } else {
        builder.append_path_with_name(source_path, archive_path)?;
    }
    Ok(())
}

fn append_symlink_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    source_path: &Path,
    archive_path: &Path,
) -> anyhow::Result<()> {
    let target = std::fs::read_link(source_path)?;
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    builder.append_link(&mut header, archive_path, target)?;
    Ok(())
}

fn handle_unsupported_entry(path: &Path, warnings: &mut Vec<TransferWarning>) {
    warnings.push(TransferWarning::skipped_unsupported_entry(path.display()));
}

fn append_source_archive<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &BundledArchiveSource,
) -> anyhow::Result<Vec<TransferWarning>> {
    let source_path = source.source_path.to_string_lossy();
    let root_name = basename_for_policy(source.source_policy, &source_path).ok_or_else(|| {
        anyhow::anyhow!(
            "transfer source path `{}` has no usable basename for multi-source transfer",
            source.source_path.display()
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
) -> anyhow::Result<Vec<TransferWarning>> {
    let mut warnings = Vec::new();
    let mut entries = archive.entries()?;
    let mut entry = entries
        .next()
        .ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
    if !entry.header().entry_type().is_file() {
        return Err(
            TransferError::source_unsupported("archive entry is not a regular file").into(),
        );
    }

    let raw_path = entry.path()?.to_path_buf();
    let rel = normalize_archive_entry_path(&raw_path)?;
    if rel != Path::new(SINGLE_FILE_ENTRY) {
        return Err(TransferError::source_unsupported(format!(
            "file archive entry path must be `{SINGLE_FILE_ENTRY}`"
        ))
        .into());
    }

    let mut header = entry.header().clone();
    builder.append_data(&mut header, root_name, &mut entry)?;
    for entry in entries {
        let mut entry = entry?;
        let raw_path = entry.path()?.to_path_buf();
        let rel = normalize_archive_entry_path(&raw_path)?;
        if !is_transfer_summary_path(&rel) {
            return Err(
                TransferError::source_unsupported("file archive contains extra entries").into(),
            );
        }
        warnings.extend(read_transfer_summary(&mut entry)?);
    }
    Ok(warnings)
}

fn append_directory_archive_to_bundle<W: Write, R: Read>(
    builder: &mut tar::Builder<W>,
    archive: &mut tar::Archive<R>,
    root_name: &str,
) -> anyhow::Result<Vec<TransferWarning>> {
    let mut warnings = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let raw_rel = entry.path()?.to_path_buf();
        let rel = normalize_archive_entry_path(&raw_rel)?;
        if is_transfer_summary_path(&rel) {
            warnings.extend(read_transfer_summary(&mut entry)?);
            continue;
        }
        let entry_type = entry.header().entry_type();
        ensure_supported_archive_entry_type(entry_type, &raw_rel)?;

        if rel.as_os_str().is_empty() {
            if !entry_type.is_dir() {
                return Err(TransferError::source_unsupported(
                    "archive file entry cannot target root",
                )
                .into());
            }
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

    Ok(warnings)
}
