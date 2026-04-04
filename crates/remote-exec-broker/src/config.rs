use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    pub targets: BTreeMap<String, TargetConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
    pub base_url: String,
    #[serde(default)]
    pub ca_pem: Option<PathBuf>,
    #[serde(default)]
    pub client_cert_pem: Option<PathBuf>,
    #[serde(default)]
    pub client_key_pem: Option<PathBuf>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    pub expected_daemon_name: Option<String>,
}

impl TargetConfig {
    fn validate_transport(&self, name: &str) -> anyhow::Result<()> {
        if self.base_url.starts_with("http://") {
            anyhow::ensure!(
                self.allow_insecure_http,
                "target `{name}` uses http://; http:// targets require allow_insecure_http = true"
            );
            return Ok(());
        }

        anyhow::ensure!(
            self.base_url.starts_with("https://"),
            "target `{name}` base_url must start with http:// or https://"
        );
        anyhow::ensure!(self.ca_pem.is_some(), "target `{name}` is missing ca_pem");
        anyhow::ensure!(
            self.client_cert_pem.is_some(),
            "target `{name}` is missing client_cert_pem"
        );
        anyhow::ensure!(
            self.client_key_pem.is_some(),
            "target `{name}` is missing client_key_pem"
        );
        Ok(())
    }
}

impl BrokerConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.targets.contains_key("local"),
            "configured target name `local` is reserved for broker-host filesystem access"
        );
        for (name, target) in &self.targets {
            target.validate_transport(name)?;
        }
        Ok(())
    }

    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        let config: Self = toml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::BrokerConfig;

    #[tokio::test]
    async fn load_rejects_reserved_local_target_name() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.local]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("configured target name `local` is reserved"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_accepts_non_reserved_target_names() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-a]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert!(config.targets.contains_key("builder-a"));
    }

    #[tokio::test]
    async fn load_rejects_http_target_without_explicit_insecure_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
expected_daemon_name = "builder-xp"
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("http:// targets require allow_insecure_http = true"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_accepts_http_target_with_explicit_insecure_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
expected_daemon_name = "builder-xp"
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert!(config.targets["builder-xp"].allow_insecure_http);
        assert_eq!(
            config.targets["builder-xp"].base_url,
            "http://127.0.0.1:8181"
        );
        assert_eq!(
            config.targets["builder-xp"].expected_daemon_name.as_deref(),
            Some("builder-xp")
        );
    }
}
