use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use remote_exec_host::{EmbeddedHostConfig, HostRuntimeConfig};
pub use remote_exec_proto::auth::HttpAuthConfig;
use remote_exec_proto::sandbox::FilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;
use serde::Deserialize;

#[cfg(test)]
mod tests;

pub use remote_exec_host::{
    HostPortForwardLimits, ProcessEnvironment, PtyMode, WindowsPtyBackendOverride, YieldTimeConfig,
    YieldTimeOperation, YieldTimeOperationConfig,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonTransport {
    #[default]
    Tls,
    Http,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    pub target: String,
    pub listen: SocketAddr,
    pub default_workdir: PathBuf,
    #[serde(default)]
    pub windows_posix_root: Option<PathBuf>,
    #[serde(default)]
    pub transport: DaemonTransport,
    #[serde(default)]
    pub http_auth: Option<HttpAuthConfig>,
    #[serde(default)]
    pub sandbox: Option<FilesystemSandbox>,
    #[serde(default = "default_enable_transfer_compression")]
    pub enable_transfer_compression: bool,
    #[serde(default)]
    pub transfer_limits: TransferLimits,
    #[serde(default = "default_max_open_sessions")]
    pub max_open_sessions: usize,
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: bool,
    #[serde(default)]
    pub pty: PtyMode,
    #[serde(default)]
    pub default_shell: Option<String>,
    #[serde(default)]
    pub yield_time: YieldTimeConfig,
    #[serde(default)]
    pub port_forward_limits: HostPortForwardLimits,
    #[serde(default)]
    pub experimental_apply_patch_target_encoding_autodetect: bool,
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

impl From<EmbeddedHostConfig> for DaemonConfig {
    fn from(value: EmbeddedHostConfig) -> Self {
        let EmbeddedHostConfig {
            target,
            default_workdir,
            windows_posix_root,
            sandbox,
            enable_transfer_compression,
            transfer_limits,
            allow_login_shell,
            pty,
            default_shell,
            yield_time,
            port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect,
            process_environment,
        } = value;
        Self {
            target,
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            default_workdir,
            windows_posix_root,
            transport: DaemonTransport::Http,
            http_auth: None,
            sandbox,
            enable_transfer_compression,
            transfer_limits,
            max_open_sessions: default_max_open_sessions(),
            allow_login_shell,
            pty,
            default_shell,
            yield_time,
            port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect,
            process_environment,
            tls: None,
        }
    }
}

impl DaemonConfig {
    fn normalized_default_workdir(&self) -> PathBuf {
        remote_exec_host::config::normalize_configured_workdir(
            &self.default_workdir,
            self.windows_posix_root.as_deref(),
        )
    }

    fn validate_http_auth(&self) -> anyhow::Result<()> {
        if let Some(http_auth) = &self.http_auth {
            http_auth.validate("")?;
        }

        Ok(())
    }

    pub fn normalize_paths(&mut self) {
        self.default_workdir = self.normalized_default_workdir();
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        HostRuntimeConfig::from(self.clone()).validate()?;
        self.validate_http_auth()?;
        crate::tls::validate_config(self)?;
        Ok(())
    }

    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        let mut config: Self = toml::from_str(&text)?;
        config.normalize_paths();
        config.validate()?;
        Ok(config)
    }
}

impl From<DaemonConfig> for HostRuntimeConfig {
    fn from(value: DaemonConfig) -> Self {
        Self {
            target: value.target,
            default_workdir: value.default_workdir,
            windows_posix_root: value.windows_posix_root,
            sandbox: value.sandbox,
            enable_transfer_compression: value.enable_transfer_compression,
            transfer_limits: value.transfer_limits,
            max_open_sessions: value.max_open_sessions,
            allow_login_shell: value.allow_login_shell,
            pty: value.pty,
            default_shell: value.default_shell,
            yield_time: value.yield_time,
            port_forward_limits: value.port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect: value
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: value.process_environment,
        }
    }
}

fn default_allow_login_shell() -> bool {
    true
}

fn default_enable_transfer_compression() -> bool {
    true
}

fn default_max_open_sessions() -> usize {
    remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS
}
