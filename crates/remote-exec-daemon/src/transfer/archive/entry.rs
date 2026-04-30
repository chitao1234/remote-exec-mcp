use std::path::{Path, PathBuf};

use remote_exec_proto::path::normalize_relative_path;

pub(super) fn normalize_archive_entry_path(raw_path: &Path) -> anyhow::Result<PathBuf> {
    normalize_relative_path(raw_path).ok_or_else(|| unsupported_archive_entry(raw_path))
}

pub(super) fn ensure_supported_archive_entry_type(
    entry_type: tar::EntryType,
    raw_path: &Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        entry_type.is_dir() || entry_type.is_file() || entry_type.is_symlink(),
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
