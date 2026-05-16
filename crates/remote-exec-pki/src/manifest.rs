use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::spec::{DevInitSpec, SubjectAltName};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPairPaths {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonManifestEntry {
    #[serde(flatten)]
    pub paths: KeyPairPaths,
    pub sans: Vec<SubjectAltName>,
}

impl DaemonManifestEntry {
    pub fn cert_pem(&self) -> &Path {
        self.paths.cert_pem.as_path()
    }

    pub fn key_pem(&self) -> &Path {
        self.paths.key_pem.as_path()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevInitManifest {
    pub created_unix_seconds: u64,
    pub out_dir: PathBuf,
    pub ca: KeyPairPaths,
    pub broker: KeyPairPaths,
    pub broker_common_name: String,
    pub daemons: BTreeMap<String, DaemonManifestEntry>,
}

pub fn build_manifest(
    spec: &DevInitSpec,
    out_dir: PathBuf,
    ca: KeyPairPaths,
    broker: KeyPairPaths,
    daemons: BTreeMap<String, KeyPairPaths>,
) -> anyhow::Result<DevInitManifest> {
    let daemon_entries = spec
        .daemon_specs
        .iter()
        .map(|daemon| {
            let paths = daemons
                .get(&daemon.target)
                .with_context(|| format!("missing daemon artifacts for `{}`", daemon.target))?;
            Ok((
                daemon.target.clone(),
                DaemonManifestEntry {
                    paths: paths.clone(),
                    sans: daemon.sans.clone(),
                },
            ))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let created_unix_seconds = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();

    Ok(DevInitManifest {
        created_unix_seconds,
        out_dir,
        ca,
        broker,
        broker_common_name: spec.broker_common_name.clone(),
        daemons: daemon_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::{DaemonManifestEntry, KeyPairPaths};
    use crate::spec::SubjectAltName;
    use std::path::PathBuf;

    #[test]
    fn daemon_manifest_entry_serializes_flat_cert_and_key_fields() {
        let entry = DaemonManifestEntry {
            paths: KeyPairPaths {
                cert_pem: PathBuf::from(r"C:\remote-exec\fixtures\builder-a.pem"),
                key_pem: PathBuf::from(r"C:\remote-exec\fixtures\builder-a-key.pem"),
            },
            sans: vec![SubjectAltName::Dns("builder-a".to_string())],
        };
        let value = serde_json::to_value(entry).expect("daemon manifest entry should serialize");

        assert_eq!(
            value.get("cert_pem").and_then(|field| field.as_str()),
            Some(r"C:\remote-exec\fixtures\builder-a.pem")
        );
        assert_eq!(
            value.get("key_pem").and_then(|field| field.as_str()),
            Some(r"C:\remote-exec\fixtures\builder-a-key.pem")
        );
        assert!(value.get("paths").is_none());
    }
}
