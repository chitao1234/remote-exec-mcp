mod bundle;
#[cfg(test)]
mod tests;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows_acl;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::manifest::KeyPairPaths;

pub use bundle::{write_broker_pair, write_ca_pair, write_daemon_pair, write_dev_init_bundle};

pub const CA_CERT_FILENAME: &str = "ca.pem";
pub const CA_KEY_FILENAME: &str = "ca.key";
const CA_PAIR_NAME: &str = "ca";
const BROKER_PAIR_NAME: &str = "broker";

fn named_pair_paths(name: &str, out_dir: &Path) -> KeyPairPaths {
    KeyPairPaths {
        cert_pem: out_dir.join(format!("{name}.pem")),
        key_pem: out_dir.join(format!("{name}.key")),
    }
}

fn validate_output_paths<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    force: bool,
) -> anyhow::Result<()> {
    for path in paths {
        if path.exists() && !force {
            bail!(
                "output path already exists: {} (rerun with --force to overwrite)",
                path.display()
            );
        }
    }
    Ok(())
}

fn write_pair(
    paths: &KeyPairPaths,
    pair: &crate::GeneratedPemPair,
    force: bool,
) -> anyhow::Result<()> {
    validate_output_paths([paths.cert_pem.as_path(), paths.key_pem.as_path()], force)?;

    let mut written_paths = Vec::new();
    write_generated_pair(paths, pair, &mut written_paths)
}

fn write_generated_pair(
    paths: &KeyPairPaths,
    pair: &crate::GeneratedPemPair,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    write_tracked_text_file(&paths.cert_pem, &pair.cert_pem, 0o644, written_paths)?;
    write_tracked_text_file(&paths.key_pem, pair.key_pem.as_str(), 0o600, written_paths)?;
    Ok(())
}

fn write_tracked_text_file(
    path: &Path,
    contents: &str,
    mode: u32,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    write_text_file(path, contents, mode, written_paths)
        .map_err(|err| err.context(format_written_paths(written_paths)))
}

fn write_text_file(
    path: &Path,
    contents: &str,
    mode: u32,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let tmp_path = temporary_path(path);

    #[cfg(unix)]
    unix::write_text_file(path, &tmp_path, contents, mode)?;

    #[cfg(not(unix))]
    {
        #[cfg(windows)]
        windows_acl::write_text_file(&tmp_path, contents, mode)
            .with_context(|| format!("writing {}", tmp_path.display()))?;

        #[cfg(not(windows))]
        {
            let _ = mode;
            fs::write(&tmp_path, contents)
                .with_context(|| format!("writing {}", tmp_path.display()))?;
        }

        if path.exists() {
            fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        }

        fs::rename(&tmp_path, path)
            .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;

        #[cfg(windows)]
        windows_acl::harden_path_if_private_key(path, mode)
            .with_context(|| format!("setting private-key ACL on {}", path.display()))?;
    }

    written_paths.push(path.to_path_buf());
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("remote-exec-pki-output"));
    path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()))
}

fn format_written_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "previously written paths: none".to_string()
    } else {
        format!(
            "previously written paths: {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
