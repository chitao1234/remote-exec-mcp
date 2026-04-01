use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::spec::{DevInitSpec, SubjectAltName};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPairPaths {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonManifestEntry {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub sans: Vec<SubjectAltName>,
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
) -> DevInitManifest {
    let daemon_entries = spec
        .daemon_specs
        .iter()
        .map(|daemon| {
            let paths = daemons
                .get(&daemon.target)
                .expect("daemon artifacts must exist for every daemon spec");
            (
                daemon.target.clone(),
                DaemonManifestEntry {
                    cert_pem: paths.cert_pem.clone(),
                    key_pem: paths.key_pem.clone(),
                    sans: daemon.sans.clone(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    DevInitManifest {
        created_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_secs(),
        out_dir,
        ca,
        broker,
        broker_common_name: spec.broker_common_name.clone(),
        daemons: daemon_entries,
    }
}

pub fn render_config_snippets(manifest: &DevInitManifest) -> String {
    let mut output = String::new();

    output.push_str("Generated files:\n");
    output.push_str(&format!("- CA cert: {}\n", manifest.ca.cert_pem.display()));
    output.push_str(&format!("- CA key: {}\n", manifest.ca.key_pem.display()));
    output.push_str(&format!(
        "- Broker cert: {}\n",
        manifest.broker.cert_pem.display()
    ));
    output.push_str(&format!(
        "- Broker key: {}\n",
        manifest.broker.key_pem.display()
    ));

    for (target, daemon) in &manifest.daemons {
        output.push_str(&format!(
            "- Daemon `{target}` cert: {}\n",
            daemon.cert_pem.display()
        ));
        output.push_str(&format!(
            "- Daemon `{target}` key: {}\n",
            daemon.key_pem.display()
        ));
        output.push_str(&format!(
            "- Daemon `{target}` SANs: {}\n",
            format_sans(&daemon.sans)
        ));
    }

    output.push_str("\nBroker config snippets:\n");
    for target in manifest.daemons.keys() {
        output.push_str(&format!(
            r#"[targets.{target}]
base_url = "https://{target}.example.com:9443"
ca_pem = "{ca_pem}"
client_cert_pem = "{broker_cert}"
client_key_pem = "{broker_key}"
expected_daemon_name = "{target}"

"#,
            target = target,
            ca_pem = manifest.ca.cert_pem.display(),
            broker_cert = manifest.broker.cert_pem.display(),
            broker_key = manifest.broker.key_pem.display(),
        ));
    }

    output.push_str("Daemon config snippets:\n");
    for (target, daemon) in &manifest.daemons {
        output.push_str(&format!(
            r#"target = "{target}"
listen = "0.0.0.0:9443"
default_workdir = "/srv/work"

[tls]
cert_pem = "{daemon_cert}"
key_pem = "{daemon_key}"
ca_pem = "{ca_pem}"

"#,
            target = target,
            daemon_cert = daemon.cert_pem.display(),
            daemon_key = daemon.key_pem.display(),
            ca_pem = manifest.ca.cert_pem.display(),
        ));
    }

    output
}

fn format_sans(sans: &[SubjectAltName]) -> String {
    sans.iter()
        .map(|san| match san {
            SubjectAltName::Dns(name) => format!("dns:{name}"),
            SubjectAltName::Ip(ip) => format!("ip:{ip}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}
