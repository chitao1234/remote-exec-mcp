use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;

use crate::{
    generate::GeneratedDevInitBundle,
    manifest::{DevInitManifest, KeyPairPaths, build_manifest},
    spec::DevInitSpec,
};

use super::{
    BROKER_PAIR_NAME, CA_PAIR_NAME, named_pair_paths, validate_output_paths, write_generated_pair,
    write_pair, write_tracked_text_file,
};

pub fn write_ca_pair(
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    write_named_pair(CA_PAIR_NAME, pair, out_dir, force)
}

pub fn write_broker_pair(
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    write_named_pair(BROKER_PAIR_NAME, pair, out_dir, force)
}

pub fn write_daemon_pair(
    target: &str,
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    write_named_pair(target, pair, out_dir, force)
}

pub fn write_dev_init_bundle(
    spec: &DevInitSpec,
    bundle: &GeneratedDevInitBundle,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<DevInitManifest> {
    let daemon_out_dir = out_dir.join("daemons");
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    fs::create_dir_all(&daemon_out_dir)
        .with_context(|| format!("creating {}", daemon_out_dir.display()))?;

    let ca = named_pair_paths(CA_PAIR_NAME, out_dir);
    let broker = named_pair_paths(BROKER_PAIR_NAME, out_dir);
    let manifest_path = out_dir.join("certs-manifest.json");
    let daemon_paths = build_daemon_paths(spec, &daemon_out_dir);
    validate_dev_init_output_paths(&ca, &broker, &daemon_paths, &manifest_path, force)?;

    let mut written_paths = Vec::new();
    write_generated_pair(&ca, &bundle.ca, &mut written_paths)?;
    write_generated_pair(&broker, &bundle.broker, &mut written_paths)?;
    write_generated_daemon_pairs(spec, bundle, &daemon_paths, &mut written_paths)?;

    let manifest = build_manifest(spec, out_dir.to_path_buf(), ca, broker, daemon_paths)?;
    write_tracked_text_file(
        &manifest_path,
        &serde_json::to_string_pretty(&manifest)?,
        0o644,
        &mut written_paths,
    )?;

    Ok(manifest)
}

fn build_daemon_paths(spec: &DevInitSpec, daemon_out_dir: &Path) -> BTreeMap<String, KeyPairPaths> {
    spec.daemon_specs
        .iter()
        .map(|daemon| {
            let target = daemon.target.clone();
            (target.clone(), named_pair_paths(&target, daemon_out_dir))
        })
        .collect()
}

fn write_named_pair(
    name: &str,
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = named_pair_paths(name, out_dir);
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

fn validate_dev_init_output_paths(
    ca: &KeyPairPaths,
    broker: &KeyPairPaths,
    daemon_paths: &BTreeMap<String, KeyPairPaths>,
    manifest_path: &Path,
    force: bool,
) -> anyhow::Result<()> {
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
            .chain(std::iter::once(manifest_path)),
        force,
    )
}

fn write_generated_daemon_pairs(
    spec: &DevInitSpec,
    bundle: &GeneratedDevInitBundle,
    daemon_paths: &BTreeMap<String, KeyPairPaths>,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    for daemon in &spec.daemon_specs {
        let target = daemon.target.as_str();
        let pem_pair = bundle
            .daemons
            .get(target)
            .with_context(|| format!("missing generated daemon bundle for `{target}`"))?;
        let paths = daemon_paths
            .get(target)
            .with_context(|| format!("missing validated daemon output paths for `{target}`"))?;
        write_generated_pair(paths, pem_pair, written_paths)?;
    }
    Ok(())
}
