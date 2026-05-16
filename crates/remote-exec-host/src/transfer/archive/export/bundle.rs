use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use remote_exec_proto::rpc::{TransferSourceType, TransferWarning};
use remote_exec_proto::transfer::TransferCompression;

use crate::error::TransferError;

use super::super::codec::{open_archive_reader, with_archive_builder};
use super::super::entry::{ensure_supported_archive_entry_type, normalize_archive_entry_path};
use super::super::summary::{
    append_transfer_summary, is_transfer_summary_path, read_transfer_summary,
};
use super::super::{BundledArchiveSource, SINGLE_FILE_ENTRY};

pub(super) fn bundle_archives_to_file(
    sources: &[BundledArchiveSource],
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<()> {
    with_archive_builder(archive_path, compression, |builder| {
        let mut warnings = Vec::new();
        for source in sources {
            warnings.extend(append_source_archive(builder, source)?);
        }
        append_transfer_summary(builder, &warnings)?;
        Ok(())
    })
}

fn append_source_archive<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &BundledArchiveSource,
) -> anyhow::Result<Vec<TransferWarning>> {
    let source_path = source.source_path.to_string_lossy();
    let root_name = source.source_policy.basename(&source_path).ok_or_else(|| {
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
        TransferSourceType::Multiple => anyhow::bail!("multi-source archives cannot be nested"),
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
