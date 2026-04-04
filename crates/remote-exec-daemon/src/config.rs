use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPtyBackendOverride {
    PortablePty,
    Winpty,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    pub target: String,
    pub listen: SocketAddr,
    pub default_workdir: PathBuf,
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: bool,
    #[serde(skip, default)]
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    pub tls: TlsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub ca_pem: PathBuf,
}

impl DaemonConfig {
    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(toml::from_str(&text)?)
    }
}

fn default_allow_login_shell() -> bool {
    true
}
