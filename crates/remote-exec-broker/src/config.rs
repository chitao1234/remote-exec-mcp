use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use remote_exec_daemon::config::{EmbeddedDaemonConfig, ProcessEnvironment, PtyMode};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    #[serde(default)]
    pub targets: BTreeMap<String, TargetConfig>,
    #[serde(default)]
    pub local: Option<LocalTargetConfig>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct LocalTargetConfig {
    pub default_workdir: PathBuf,
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: bool,
    #[serde(default)]
    pub pty: PtyMode,
    #[serde(default)]
    pub default_shell: Option<String>,
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

impl LocalTargetConfig {
    pub fn embedded_daemon_config(&self) -> EmbeddedDaemonConfig {
        EmbeddedDaemonConfig {
            target: "local".to_string(),
            default_workdir: self.default_workdir.clone(),
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.clone(),
            process_environment: ProcessEnvironment::capture_current(),
        }
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

fn default_allow_login_shell() -> bool {
    true
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
    async fn load_accepts_local_only_broker_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            format!(
                "[local]\ndefault_workdir = {}\nallow_login_shell = false\n",
                toml::Value::String(dir.path().display().to_string())
            ),
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert!(config.targets.is_empty());
        assert_eq!(
            config.local.as_ref().map(|local| &local.default_workdir),
            Some(&dir.path().to_path_buf())
        );
        assert_eq!(
            config.local.as_ref().map(|local| local.allow_login_shell),
            Some(false)
        );
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
