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
    pub ca_pem: PathBuf,
    pub client_cert_pem: PathBuf,
    pub client_key_pem: PathBuf,
    pub expected_daemon_name: Option<String>,
}

impl BrokerConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.targets.contains_key("local"),
            "configured target name `local` is reserved for broker-host filesystem access"
        );
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
}
