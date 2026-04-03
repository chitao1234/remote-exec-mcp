use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{
    generate::GeneratedDevInitBundle,
    manifest::{DevInitManifest, KeyPairPaths, build_manifest},
    spec::DevInitSpec,
};

pub fn write_dev_init_bundle(
    spec: &DevInitSpec,
    bundle: &GeneratedDevInitBundle,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<DevInitManifest> {
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    fs::create_dir_all(out_dir.join("daemons"))
        .with_context(|| format!("creating {}", out_dir.join("daemons").display()))?;

    let ca = KeyPairPaths {
        cert_pem: out_dir.join("ca.pem"),
        key_pem: out_dir.join("ca.key"),
    };
    let broker = KeyPairPaths {
        cert_pem: out_dir.join("broker.pem"),
        key_pem: out_dir.join("broker.key"),
    };
    let manifest_path = out_dir.join("certs-manifest.json");

    let mut daemon_paths = BTreeMap::new();
    for target in spec
        .daemon_specs
        .iter()
        .map(|daemon| daemon.target.as_str())
    {
        daemon_paths.insert(
            target.to_string(),
            KeyPairPaths {
                cert_pem: out_dir.join("daemons").join(format!("{target}.pem")),
                key_pem: out_dir.join("daemons").join(format!("{target}.key")),
            },
        );
    }

    validate_output_paths(
        std::iter::once(ca.cert_pem.as_path())
            .chain(std::iter::once(ca.key_pem.as_path()))
            .chain(std::iter::once(broker.cert_pem.as_path()))
            .chain(std::iter::once(broker.key_pem.as_path()))
            .chain(
                daemon_paths
                    .values()
                    .flat_map(|paths| [paths.cert_pem.as_path(), paths.key_pem.as_path()]),
            )
            .chain(std::iter::once(manifest_path.as_path())),
        force,
    )?;

    let mut written_paths = Vec::new();

    write_text_file(
        &ca.cert_pem,
        &bundle.ca.cert_pem,
        force,
        0o644,
        &mut written_paths,
    )
    .map_err(|err| err.context(format_written_paths(&written_paths)))?;
    write_text_file(
        &ca.key_pem,
        &bundle.ca.key_pem,
        force,
        0o600,
        &mut written_paths,
    )
    .map_err(|err| err.context(format_written_paths(&written_paths)))?;
    write_text_file(
        &broker.cert_pem,
        &bundle.broker.cert_pem,
        force,
        0o644,
        &mut written_paths,
    )
    .map_err(|err| err.context(format_written_paths(&written_paths)))?;
    write_text_file(
        &broker.key_pem,
        &bundle.broker.key_pem,
        force,
        0o600,
        &mut written_paths,
    )
    .map_err(|err| err.context(format_written_paths(&written_paths)))?;

    for target in spec
        .daemon_specs
        .iter()
        .map(|daemon| daemon.target.as_str())
    {
        let pem_pair = bundle
            .daemons
            .get(target)
            .with_context(|| format!("missing generated daemon bundle for `{target}`"))?;
        let paths = daemon_paths
            .get(target)
            .expect("validated daemon paths must exist");
        write_text_file(
            &paths.cert_pem,
            &pem_pair.cert_pem,
            force,
            0o644,
            &mut written_paths,
        )
        .map_err(|err| err.context(format_written_paths(&written_paths)))?;
        write_text_file(
            &paths.key_pem,
            &pem_pair.key_pem,
            force,
            0o600,
            &mut written_paths,
        )
        .map_err(|err| err.context(format_written_paths(&written_paths)))?;
    }

    let manifest = build_manifest(spec, out_dir.to_path_buf(), ca, broker, daemon_paths);
    write_text_file(
        &manifest_path,
        &serde_json::to_string_pretty(&manifest)?,
        force,
        0o644,
        &mut written_paths,
    )
    .map_err(|err| err.context(format_written_paths(&written_paths)))?;

    Ok(manifest)
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

fn write_text_file(
    path: &Path,
    contents: &str,
    force: bool,
    _mode: u32,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if path.exists() && !force {
        bail!(
            "output path already exists: {} (rerun with --force to overwrite)",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let tmp_path = temporary_path(path);
    fs::write(&tmp_path, contents).with_context(|| format!("writing {}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(_mode))
            .with_context(|| format!("setting permissions on {}", tmp_path.display()))?;
    }

    if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }

    fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
    written_paths.push(path.to_path_buf());
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path.file_name().expect("paths must have file names");
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
