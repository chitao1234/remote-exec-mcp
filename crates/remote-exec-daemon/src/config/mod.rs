use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use remote_exec_host::{EmbeddedHostConfig, HostRuntimeConfig};
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
pub struct HttpAuthConfig {
    pub bearer_token: String,
    #[serde(skip)]
    pub expected_authorization: String,
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
    pub host: EmbeddedHostConfig,
}

impl EmbeddedDaemonConfig {
    pub fn into_host_config(self) -> EmbeddedHostConfig {
        self.host
    }

    pub fn into_daemon_config(self) -> DaemonConfig {
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
        } = self.host;
        DaemonConfig {
            target,
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            default_workdir,
            windows_posix_root,
            transport: DaemonTransport::Http,
            http_auth: None,
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
            tls: None,
        }
    }
}

impl From<EmbeddedHostConfig> for EmbeddedDaemonConfig {
    fn from(value: EmbeddedHostConfig) -> Self {
        Self { host: value }
    }
}

impl From<EmbeddedHostConfig> for DaemonConfig {
    fn from(value: EmbeddedHostConfig) -> Self {
        EmbeddedDaemonConfig::from(value).into_daemon_config()
    }
}

impl DaemonConfig {
    pub fn host_runtime_config(&self) -> HostRuntimeConfig {
        self.clone().into()
    }

    pub fn into_host_runtime_config(self) -> HostRuntimeConfig {
        self.into()
    }

    fn normalized_default_workdir(&self) -> PathBuf {
        normalize_configured_workdir(&self.default_workdir, self.windows_posix_root.as_deref())
    }

    fn validate_http_auth(&self) -> anyhow::Result<()> {
        if let Some(http_auth) = &self.http_auth {
            http_auth.validate()?;
        }

        Ok(())
    }

    pub fn normalize_paths(&mut self) {
        self.default_workdir = self.normalized_default_workdir();
    }

    pub fn prepare_runtime_fields(&mut self) {
        if let Some(http_auth) = &mut self.http_auth {
            http_auth.prepare_runtime_fields();
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.host_runtime_config().validate()?;
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
        config.prepare_runtime_fields();
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

pub fn normalize_configured_workdir(path: &Path, windows_posix_root: Option<&Path>) -> PathBuf {
    remote_exec_host::config::normalize_configured_workdir(path, windows_posix_root)
}

impl HttpAuthConfig {
    fn prepare_runtime_fields(&mut self) {
        self.expected_authorization = format!("Bearer {}", self.bearer_token);
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.bearer_token.is_empty(),
            "http_auth.bearer_token must not be empty"
        );
        anyhow::ensure!(
            !self.bearer_token.chars().any(char::is_whitespace),
            "http_auth.bearer_token must not contain whitespace"
        );
        Ok(())
    }
}

fn default_allow_login_shell() -> bool {
    true
}

fn default_enable_transfer_compression() -> bool {
    true
}
