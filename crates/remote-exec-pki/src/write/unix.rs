use std::{fs, path::Path};

use anyhow::Context;

pub(super) fn write_text_file(
    path: &Path,
    tmp_path: &Path,
    contents: &str,
    mode: u32,
) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::write(tmp_path, contents).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(tmp_path, path)
        .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    Ok(())
}
