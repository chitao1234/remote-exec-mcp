use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use remote_exec_proto::sandbox::FilesystemSandbox;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPtyBackendOverride {
    PortablePty,
    Winpty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PtyMode {
    #[default]
    Auto,
    Conpty,
    Winpty,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonTransport {
    #[default]
    Tls,
    Http,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessEnvironment {
    path: Option<OsString>,
    comspec: Option<String>,
    vars: Vec<(OsString, OsString)>,
}

impl ProcessEnvironment {
    pub fn capture_current() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            comspec: std::env::var("COMSPEC").ok(),
            vars: std::env::vars_os().collect(),
        }
    }

    pub fn path(&self) -> Option<&OsStr> {
        self.path.as_deref()
    }

    pub fn comspec(&self) -> Option<&str> {
        self.comspec.as_deref()
    }

    pub fn vars(&self) -> &[(OsString, OsString)] {
        &self.vars
    }

    pub fn var_os(&self, key: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .find(|(existing_key, _)| env_key_matches(existing_key, key))
            .map(|(_, value)| value.as_os_str())
    }

    pub fn set_var(&mut self, key: &str, value: Option<OsString>) {
        self.vars
            .retain(|(existing_key, _)| !env_key_matches(existing_key, key));

        if let Some(value) = value {
            self.vars.push((OsString::from(key), value.clone()));
            if key.eq_ignore_ascii_case("PATH") {
                self.path = Some(value.clone());
            }
            if key.eq_ignore_ascii_case("COMSPEC") {
                self.comspec = Some(value.to_string_lossy().into_owned());
            }
        } else {
            if key.eq_ignore_ascii_case("PATH") {
                self.path = None;
            }
            if key.eq_ignore_ascii_case("COMSPEC") {
                self.comspec = None;
            }
        }
    }
}

fn env_key_matches(existing_key: &OsStr, requested_key: &str) -> bool {
    #[cfg(windows)]
    {
        existing_key
            .to_string_lossy()
            .eq_ignore_ascii_case(requested_key)
    }

    #[cfg(not(windows))]
    {
        existing_key == OsStr::new(requested_key)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    pub target: String,
    pub listen: SocketAddr,
    pub default_workdir: PathBuf,
    #[serde(default)]
    pub transport: DaemonTransport,
    #[serde(default)]
    pub sandbox: Option<FilesystemSandbox>,
    #[serde(default = "default_enable_transfer_compression")]
    pub enable_transfer_compression: bool,
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: bool,
    #[serde(default)]
    pub pty: PtyMode,
    #[serde(default)]
    pub default_shell: Option<String>,
    #[serde(skip, default = "ProcessEnvironment::capture_current")]
    pub process_environment: ProcessEnvironment,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub ca_pem: PathBuf,
    #[serde(default)]
    pub pinned_client_cert_pem: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct EmbeddedDaemonConfig {
    pub target: String,
    pub default_workdir: PathBuf,
    pub sandbox: Option<FilesystemSandbox>,
    pub enable_transfer_compression: bool,
    pub allow_login_shell: bool,
    pub pty: PtyMode,
    pub default_shell: Option<String>,
    pub process_environment: ProcessEnvironment,
}

impl EmbeddedDaemonConfig {
    pub fn into_daemon_config(self) -> DaemonConfig {
        DaemonConfig {
            target: self.target,
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            default_workdir: self.default_workdir,
            transport: DaemonTransport::Http,
            sandbox: self.sandbox,
            enable_transfer_compression: self.enable_transfer_compression,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell,
            process_environment: self.process_environment,
            tls: None,
        }
    }
}

impl DaemonConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if matches!(self.transport, DaemonTransport::Tls) {
            anyhow::ensure!(
                self.tls.is_some(),
                "tls config is required when transport = \"tls\""
            );
        }
        if matches!(self.transport, DaemonTransport::Http)
            && self
                .tls
                .as_ref()
                .is_some_and(|tls| tls.pinned_client_cert_pem.is_some())
        {
            anyhow::bail!("pinned_client_cert_pem requires transport = \"tls\"");
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{DaemonConfig, DaemonTransport};

    #[tokio::test]
    async fn load_accepts_http_transport_without_tls_block() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = "/tmp"
transport = "http"
"#,
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert!(matches!(config.transport, DaemonTransport::Http));
        assert!(config.tls.is_none());
    }

    #[tokio::test]
    async fn load_rejects_default_tls_transport_without_tls_block() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = "/tmp"
"#,
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("tls config is required when transport = \"tls\""),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_accepts_tls_transport_with_pinned_client_cert() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = "/tmp"

[tls]
cert_pem = "/tmp/daemon.pem"
key_pem = "/tmp/daemon.key"
ca_pem = "/tmp/ca.pem"
pinned_client_cert_pem = "/tmp/broker.pem"
"#,
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config
                .tls
                .as_ref()
                .and_then(|tls| tls.pinned_client_cert_pem.as_ref()),
            Some(&PathBuf::from("/tmp/broker.pem"))
        );
    }

    #[tokio::test]
    async fn load_rejects_pinned_client_cert_for_http_transport() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = "/tmp"
transport = "http"

[tls]
cert_pem = "/tmp/daemon.pem"
key_pem = "/tmp/daemon.key"
ca_pem = "/tmp/ca.pem"
pinned_client_cert_pem = "/tmp/broker.pem"
"#,
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("pinned_client_cert_pem requires transport = \"tls\""),
            "unexpected error: {err}"
        );
    }
}
