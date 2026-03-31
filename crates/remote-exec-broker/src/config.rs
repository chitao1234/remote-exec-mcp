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
    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(toml::from_str(&text)?)
    }
}
