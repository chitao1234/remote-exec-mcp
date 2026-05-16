use remote_exec_pki::{DevInitManifest, SubjectAltName};

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

pub(crate) fn render_dev_init_output(manifest: &DevInitManifest) -> String {
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
            daemon.cert_pem().display()
        ));
        output.push_str(&format!(
            "- Daemon `{target}` key: {}\n",
            daemon.key_pem().display()
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
# Set this to the daemon HTTPS endpoint.
# base_url = {base_url}
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
# Set this to the daemon bind address.
# listen = {listen}
default_workdir = {default_workdir}

[tls]
cert_pem = {daemon_cert}
key_pem = {daemon_key}
ca_pem = {ca_pem}
# Optional exact broker leaf certificate pin.
# pinned_client_cert_pem = {broker_cert}

"#,
            target = toml_string(target),
            listen = toml_string("0.0.0.0:9443"),
            default_workdir = toml_string("/srv/work"),
            daemon_cert = toml_string(&daemon.cert_pem().display().to_string()),
            daemon_key = toml_string(&daemon.key_pem().display().to_string()),
            ca_pem = toml_string(&manifest.ca.cert_pem.display().to_string()),
            broker_cert = toml_string(&manifest.broker.cert_pem.display().to_string()),
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
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use remote_exec_pki::{DaemonManifestEntry, DevInitManifest, KeyPairPaths, SubjectAltName};

    use super::render_dev_init_output;

    fn sample_manifest() -> DevInitManifest {
        let mut daemons = BTreeMap::new();
        daemons.insert(
            "builder-a".to_string(),
            DaemonManifestEntry {
                paths: KeyPairPaths {
                    cert_pem: PathBuf::from(r"C:\remote-exec\fixtures\builder-a.pem"),
                    key_pem: PathBuf::from(r"C:\remote-exec\fixtures\builder-a-key.pem"),
                },
                sans: vec![SubjectAltName::Dns("builder-a".to_string())],
            },
        );

        DevInitManifest {
            created_unix_seconds: 0,
            out_dir: PathBuf::from(r"C:\remote-exec\fixtures"),
            ca: KeyPairPaths {
                cert_pem: PathBuf::from(r"C:\remote-exec\fixtures\ca.pem"),
                key_pem: PathBuf::from(r"C:\remote-exec\fixtures\ca-key.pem"),
            },
            broker: KeyPairPaths {
                cert_pem: PathBuf::from(r"C:\remote-exec\fixtures\broker.pem"),
                key_pem: PathBuf::from(r"C:\remote-exec\fixtures\broker-key.pem"),
            },
            broker_common_name: "remote-exec-broker".to_string(),
            daemons,
        }
    }

    #[test]
    fn broker_snippet_escapes_windows_paths_for_toml() {
        let snippets = render_dev_init_output(&sample_manifest());
        let broker_section = snippets
            .split("Broker config snippets:\n")
            .nth(1)
            .and_then(|rest| rest.split("Daemon config snippets:\n").next())
            .expect("broker snippet section");

        let uncommented = uncomment_placeholder_lines(broker_section);
        let parsed = uncommented
            .parse::<toml::Table>()
            .expect("broker snippet should parse as TOML");

        assert_eq!(
            parsed["targets"]["builder-a"]["base_url"].as_str(),
            Some("https://builder-a.example.com:9443")
        );
        assert_eq!(
            parsed["targets"]["builder-a"]["ca_pem"].as_str(),
            Some(r"C:\remote-exec\fixtures\ca.pem")
        );
        assert_eq!(
            parsed["targets"]["builder-a"]["client_cert_pem"].as_str(),
            Some(r"C:\remote-exec\fixtures\broker.pem")
        );
        assert_eq!(
            parsed["targets"]["builder-a"]["client_key_pem"].as_str(),
            Some(r"C:\remote-exec\fixtures\broker-key.pem")
        );
    }

    #[test]
    fn daemon_snippet_escapes_windows_paths_for_toml() {
        let snippets = render_dev_init_output(&sample_manifest());
        let daemon_section = snippets
            .split("Daemon config snippets:\n")
            .nth(1)
            .expect("daemon snippet section");

        let uncommented = uncomment_placeholder_lines(daemon_section);
        let parsed = uncommented
            .parse::<toml::Table>()
            .expect("daemon snippet should parse as TOML");

        assert_eq!(parsed["listen"].as_str(), Some("0.0.0.0:9443"));
        assert_eq!(
            parsed["tls"]["cert_pem"].as_str(),
            Some(r"C:\remote-exec\fixtures\builder-a.pem")
        );
        assert_eq!(
            parsed["tls"]["key_pem"].as_str(),
            Some(r"C:\remote-exec\fixtures\builder-a-key.pem")
        );
        assert_eq!(
            parsed["tls"]["ca_pem"].as_str(),
            Some(r"C:\remote-exec\fixtures\ca.pem")
        );
    }

    #[test]
    fn endpoint_and_bind_placeholders_are_commented() {
        let snippets = render_dev_init_output(&sample_manifest());
        assert!(snippets.contains("# base_url = \"https://builder-a.example.com:9443\""));
        assert!(snippets.contains("# listen = \"0.0.0.0:9443\""));
        assert!(!snippets.contains("\nbase_url = \"https://builder-a.example.com:9443\""));
        assert!(!snippets.contains("\nlisten = \"0.0.0.0:9443\""));
    }

    fn uncomment_placeholder_lines(input: &str) -> String {
        input
            .lines()
            .map(|line| {
                if let Some(rest) = line.strip_prefix("# base_url = ") {
                    format!("base_url = {rest}")
                } else if let Some(rest) = line.strip_prefix("# listen = ") {
                    format!("listen = {rest}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
