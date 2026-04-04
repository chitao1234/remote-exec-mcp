use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::spec::{DevInitSpec, SubjectAltName};

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

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
base_url = {base_url}
ca_pem = {ca_pem}
client_cert_pem = {broker_cert}
client_key_pem = {broker_key}
expected_daemon_name = {expected_daemon_name}

"#,
            target = target,
            base_url = toml_string(&format!("https://{target}.example.com:9443")),
            ca_pem = toml_string(&manifest.ca.cert_pem.display().to_string()),
            broker_cert = toml_string(&manifest.broker.cert_pem.display().to_string()),
            broker_key = toml_string(&manifest.broker.key_pem.display().to_string()),
            expected_daemon_name = toml_string(target),
        ));
    }

    output.push_str("Daemon config snippets:\n");
    for (target, daemon) in &manifest.daemons {
        output.push_str(&format!(
            r#"target = {target}
listen = {listen}
default_workdir = {default_workdir}

[tls]
cert_pem = {daemon_cert}
key_pem = {daemon_key}
ca_pem = {ca_pem}

"#,
            target = toml_string(target),
            listen = toml_string("0.0.0.0:9443"),
            default_workdir = toml_string("/srv/work"),
            daemon_cert = toml_string(&daemon.cert_pem.display().to_string()),
            daemon_key = toml_string(&daemon.key_pem.display().to_string()),
            ca_pem = toml_string(&manifest.ca.cert_pem.display().to_string()),
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

#[cfg(test)]
mod tests {
    use super::{DaemonManifestEntry, DevInitManifest, KeyPairPaths, render_config_snippets};
    use crate::spec::SubjectAltName;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn sample_manifest() -> DevInitManifest {
        let mut daemons = BTreeMap::new();
        daemons.insert(
            "builder-a".to_string(),
            DaemonManifestEntry {
                cert_pem: PathBuf::from(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\builder-a.pem"),
                key_pem: PathBuf::from(
                    r"C:\Users\chi\AppData\Local\Temp\.tmp-work\builder-a-key.pem",
                ),
                sans: vec![SubjectAltName::Dns("builder-a".to_string())],
            },
        );

        DevInitManifest {
            created_unix_seconds: 0,
            out_dir: PathBuf::from(r"C:\Users\chi\AppData\Local\Temp\.tmp-work"),
            ca: KeyPairPaths {
                cert_pem: PathBuf::from(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\ca.pem"),
                key_pem: PathBuf::from(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\ca-key.pem"),
            },
            broker: KeyPairPaths {
                cert_pem: PathBuf::from(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\broker.pem"),
                key_pem: PathBuf::from(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\broker-key.pem"),
            },
            broker_common_name: "remote-exec-broker".to_string(),
            daemons,
        }
    }

    #[test]
    fn broker_snippet_escapes_windows_paths_for_toml() {
        let snippets = render_config_snippets(&sample_manifest());
        let broker_section = snippets
            .split("Broker config snippets:\n")
            .nth(1)
            .and_then(|rest| rest.split("Daemon config snippets:\n").next())
            .expect("broker snippet section");

        let parsed = broker_section
            .parse::<toml::Table>()
            .expect("broker snippet should parse as TOML");

        assert_eq!(
            parsed["targets"]["builder-a"]["ca_pem"].as_str(),
            Some(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\ca.pem")
        );
        assert_eq!(
            parsed["targets"]["builder-a"]["client_cert_pem"].as_str(),
            Some(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\broker.pem")
        );
        assert_eq!(
            parsed["targets"]["builder-a"]["client_key_pem"].as_str(),
            Some(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\broker-key.pem")
        );
    }

    #[test]
    fn daemon_snippet_escapes_windows_paths_for_toml() {
        let snippets = render_config_snippets(&sample_manifest());
        let daemon_section = snippets
            .split("Daemon config snippets:\n")
            .nth(1)
            .expect("daemon snippet section");

        let parsed = daemon_section
            .parse::<toml::Table>()
            .expect("daemon snippet should parse as TOML");

        assert_eq!(
            parsed["tls"]["cert_pem"].as_str(),
            Some(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\builder-a.pem")
        );
        assert_eq!(
            parsed["tls"]["key_pem"].as_str(),
            Some(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\builder-a-key.pem")
        );
        assert_eq!(
            parsed["tls"]["ca_pem"].as_str(),
            Some(r"C:\Users\chi\AppData\Local\Temp\.tmp-work\ca.pem")
        );
    }
}
