use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use remote_exec_daemon::config::{
    EmbeddedDaemonConfig, ProcessEnvironment, PtyMode, YieldTimeConfig,
};
use remote_exec_proto::sandbox::FilesystemSandbox;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    #[serde(default)]
    pub mcp: McpServerConfig,
    #[serde(default)]
    pub targets: BTreeMap<String, TargetConfig>,
    #[serde(default)]
    pub local: Option<LocalTargetConfig>,
    #[serde(default)]
    pub host_sandbox: Option<FilesystemSandbox>,
    #[serde(default = "default_enable_transfer_compression")]
    pub enable_transfer_compression: bool,
    #[serde(default)]
    pub disable_structured_content: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpServerConfig {
    #[default]
    Stdio,
    StreamableHttp {
        listen: SocketAddr,
        #[serde(default = "default_streamable_http_path")]
        path: String,
        #[serde(default = "default_streamable_http_stateful")]
        stateful: bool,
        #[serde(default = "default_streamable_http_sse_keep_alive_ms")]
        sse_keep_alive_ms: Option<u64>,
        #[serde(default = "default_streamable_http_sse_retry_ms")]
        sse_retry_ms: Option<u64>,
    },
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
    #[serde(default)]
    pub skip_server_name_verification: bool,
    #[serde(default)]
    pub pinned_server_cert_pem: Option<PathBuf>,
    pub expected_daemon_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetTransportKind {
    Http,
    Https,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalTargetConfig {
    pub default_workdir: PathBuf,
    #[serde(default)]
    pub windows_posix_root: Option<PathBuf>,
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: bool,
    #[serde(default)]
    pub pty: PtyMode,
    #[serde(default)]
    pub default_shell: Option<String>,
    #[serde(default)]
    pub yield_time: YieldTimeConfig,
    #[serde(default)]
    pub experimental_apply_patch_target_encoding_autodetect: bool,
}

impl TargetConfig {
    pub(crate) fn validated_transport(&self, name: &str) -> anyhow::Result<TargetTransportKind> {
        if self.base_url.starts_with("http://") {
            anyhow::ensure!(
                self.allow_insecure_http,
                "target `{name}` uses http://; http:// targets require allow_insecure_http = true"
            );
            anyhow::ensure!(
                !self.skip_server_name_verification,
                "target `{name}` cannot set skip_server_name_verification for http:// targets"
            );
            anyhow::ensure!(
                self.pinned_server_cert_pem.is_none(),
                "target `{name}` cannot set pinned_server_cert_pem for http:// targets"
            );
            return Ok(TargetTransportKind::Http);
        }

        anyhow::ensure!(
            self.base_url.starts_with("https://"),
            "target `{name}` base_url must start with http:// or https://"
        );
        crate::broker_tls::ensure_https_target_supported(name)?;
        anyhow::ensure!(self.ca_pem.is_some(), "target `{name}` is missing ca_pem");
        anyhow::ensure!(
            self.client_cert_pem.is_some(),
            "target `{name}` is missing client_cert_pem"
        );
        anyhow::ensure!(
            self.client_key_pem.is_some(),
            "target `{name}` is missing client_key_pem"
        );
        Ok(TargetTransportKind::Https)
    }
}

impl LocalTargetConfig {
    pub fn embedded_daemon_config(
        &self,
        sandbox: Option<FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> EmbeddedDaemonConfig {
        EmbeddedDaemonConfig {
            target: "local".to_string(),
            default_workdir: self.default_workdir.clone(),
            windows_posix_root: self.windows_posix_root.clone(),
            sandbox,
            enable_transfer_compression,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.clone(),
            yield_time: self.yield_time,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: ProcessEnvironment::capture_current(),
        }
    }
}

impl McpServerConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Stdio => Ok(()),
            Self::StreamableHttp { path, .. } => {
                anyhow::ensure!(
                    path.starts_with('/'),
                    "streamable_http MCP path must start with `/`"
                );
                Ok(())
            }
        }
    }
}

impl BrokerConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        self.mcp.validate()?;
        anyhow::ensure!(
            !self.targets.contains_key("local"),
            "configured target name `local` is reserved for broker-host filesystem access"
        );
        for (name, target) in &self.targets {
            target.validated_transport(name)?;
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

fn default_enable_transfer_compression() -> bool {
    true
}

fn default_streamable_http_path() -> String {
    "/mcp".to_string()
}

fn default_streamable_http_stateful() -> bool {
    true
}

fn default_streamable_http_sse_keep_alive_ms() -> Option<u64> {
    Some(15_000)
}

fn default_streamable_http_sse_retry_ms() -> Option<u64> {
    Some(3_000)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use std::path::PathBuf;

    use super::{BrokerConfig, McpServerConfig};

    fn valid_target_config(name: &str) -> String {
        if cfg!(feature = "broker-tls") {
            format!(
                r#"[targets.{name}]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#
            )
        } else {
            format!(
                r#"[targets.{name}]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
"#
            )
        }
    }

    #[tokio::test]
    async fn load_rejects_reserved_local_target_name() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(&config_path, valid_target_config("local"))
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
        tokio::fs::write(&config_path, valid_target_config("builder-a"))
            .await
            .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert!(config.targets.contains_key("builder-a"));
    }

    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn load_rejects_https_targets_when_broker_tls_feature_disabled() {
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

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}"
        );
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
        assert!(!config.disable_structured_content);
        assert!(matches!(config.mcp, McpServerConfig::Stdio));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn load_accepts_local_windows_posix_root() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            format!(
                "[local]\ndefault_workdir = {}\nwindows_posix_root = \"C:\\\\msys64\"\n",
                toml::Value::String(dir.path().display().to_string())
            ),
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config
                .local
                .as_ref()
                .and_then(|local| local.windows_posix_root.as_ref()),
            Some(&PathBuf::from(r"C:\msys64"))
        );
    }

    #[tokio::test]
    async fn load_accepts_local_apply_patch_encoding_autodetect() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            format!(
                "[local]\ndefault_workdir = {}\nexperimental_apply_patch_target_encoding_autodetect = true\n",
                toml::Value::String(dir.path().display().to_string())
            ),
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config
                .local
                .as_ref()
                .map(|local| local.experimental_apply_patch_target_encoding_autodetect),
            Some(true)
        );
    }

    #[tokio::test]
    async fn load_accepts_disabling_structured_content() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            format!(
                "disable_structured_content = true\n\n{}",
                valid_target_config("builder-a")
            ),
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert!(config.disable_structured_content);
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

    #[tokio::test]
    async fn load_rejects_server_name_skip_for_http_target() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
skip_server_name_verification = true
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot set skip_server_name_verification"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_rejects_server_cert_pin_for_http_target() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
pinned_server_cert_pem = "/tmp/pin.pem"
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot set pinned_server_cert_pem"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_accepts_streamable_http_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "/rpc"
stateful = false
sse_keep_alive_ms = 0
sse_retry_ms = 1000
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        match config.mcp {
            McpServerConfig::StreamableHttp {
                listen,
                path,
                stateful,
                sse_keep_alive_ms,
                sse_retry_ms,
            } => {
                assert_eq!(listen, "127.0.0.1:8787".parse().unwrap());
                assert_eq!(path, "/rpc");
                assert!(!stateful);
                assert_eq!(sse_keep_alive_ms, Some(0));
                assert_eq!(sse_retry_ms, Some(1000));
            }
            other => panic!("unexpected MCP config: {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_rejects_streamable_http_path_without_leading_slash() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "mcp"
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("streamable_http MCP path must start with `/`"),
            "unexpected error: {err}"
        );
    }
}
