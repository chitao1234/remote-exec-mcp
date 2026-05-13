use std::io::Write;
use std::path::{Path, PathBuf};

use remote_exec_proto::rpc::{TransferSourceType, TransferSymlinkMode, TransferWarning};

use crate::error::TransferError;

use super::super::SINGLE_FILE_ENTRY;
use super::super::codec::{with_archive_builder, with_archive_writer};
use super::super::exclude_matcher::{ExcludeMatcher, normalize_relative_path};
use super::super::summary::append_transfer_summary;
use super::PreparedExport;

pub(super) async fn write_prepared_export_to_file(
    prepared: PreparedExport,
    archive_path: PathBuf,
    compression: remote_exec_proto::transfer::TransferCompression,
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

pub(super) async fn write_prepared_export_to_writer<W>(
    prepared: PreparedExport,
    writer: W,
    compression: remote_exec_proto::transfer::TransferCompression,
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
