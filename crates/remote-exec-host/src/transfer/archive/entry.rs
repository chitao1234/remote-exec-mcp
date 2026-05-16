use std::path::{Component, Path, PathBuf};

use crate::error::TransferError;

pub(super) fn normalize_archive_entry_path(raw_path: &Path) -> anyhow::Result<PathBuf> {
    let mut normalized = PathBuf::new();

    for component in raw_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                return Err(
                    TransferError::source_unsupported("archive path escapes destination").into(),
                );
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(
                    TransferError::source_unsupported("archive path must be relative").into(),
                );
            }
        }
    }

    Ok(normalized)
}

pub(super) fn ensure_supported_archive_entry_type(
    entry_type: tar::EntryType,
    raw_path: &Path,
) -> anyhow::Result<()> {
    if !(entry_type.is_dir() || entry_type.is_file() || entry_type.is_symlink()) {
        return Err(unsupported_archive_entry(raw_path));
    }
    Ok(())
}

fn unsupported_archive_entry(raw_path: &Path) -> anyhow::Error {
    TransferError::source_unsupported(format!(
        "archive contains unsupported entry `{}`",
        raw_path.display()
    ))
    .into()
}
