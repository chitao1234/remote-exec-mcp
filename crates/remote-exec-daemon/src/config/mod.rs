use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;

use anyhow::Context;
use remote_exec_host::HostRuntimeConfig;
use remote_exec_host::config::HostRuntimeConfigSource;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonConnectionMode {
    #[default]
    Listen,
    Reverse,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    pub target: String,
    #[serde(default = "default_daemon_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub connection_mode: DaemonConnectionMode,
    #[serde(default)]
    pub reverse: Option<ReverseConnectionConfig>,
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
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReverseConnectionConfig {
    pub broker_addr: String,
    #[serde(default)]
    pub transport: DaemonTransport,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default = "default_reverse_min_idle_connections")]
    pub min_idle_connections: usize,
    #[serde(default = "default_reverse_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_reverse_reconnect_min_ms")]
    pub reconnect_min_ms: u64,
    #[serde(default = "default_reverse_reconnect_max_ms")]
    pub reconnect_max_ms: u64,
    #[serde(default = "default_reverse_registration_timeout_ms")]
    pub registration_timeout_ms: u64,
    #[serde(default)]
    pub tls: Option<ReverseClientTlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReverseClientTlsConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub ca_pem: PathBuf,
    pub server_name: String,
    #[serde(default)]
    pub pinned_server_cert_pem: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ValidatedDaemonConfig(DaemonConfig);

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub ca_pem: PathBuf,
    #[serde(default)]
    pub pinned_client_cert_pem: Option<PathBuf>,
}

impl From<HostRuntimeConfig> for DaemonConfig {
    fn from(value: HostRuntimeConfig) -> Self {
        let HostRuntimeConfig {
            target,
            default_workdir,
            windows_posix_root,
            sandbox,
            enable_transfer_compression,
            transfer_limits,
            max_open_sessions,
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
            connection_mode: DaemonConnectionMode::Listen,
            reverse: None,
            default_workdir,
            windows_posix_root,
            transport: DaemonTransport::Http,
            http_auth: None,
            sandbox,
            enable_transfer_compression,
            transfer_limits,
            max_open_sessions,
            allow_login_shell,
            pty,
            default_shell,
            yield_time,
            port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect,
            process_environment,
            tls: None,
            request_timeout_ms: default_request_timeout_ms(),
        }
    }
}

impl DaemonConfig {
    fn host_runtime_config_source(&self) -> HostRuntimeConfigSource<'_> {
        HostRuntimeConfigSource {
            target: &self.target,
            default_workdir: &self.default_workdir,
            windows_posix_root: self.windows_posix_root.as_deref(),
            sandbox: self.sandbox.as_ref(),
            enable_transfer_compression: self.enable_transfer_compression,
            transfer_limits: self.transfer_limits,
            max_open_sessions: self.max_open_sessions,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.as_deref(),
            yield_time: self.yield_time,
            port_forward_limits: self.port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: &self.process_environment,
        }
    }

    fn validate_http_auth(&self) -> anyhow::Result<()> {
        if let Some(http_auth) = &self.http_auth {
            http_auth.validate("")?;
        }

        Ok(())
    }

    pub fn normalize_paths(&mut self) {
        let mut host_config = HostRuntimeConfig::from_source(self.host_runtime_config_source());
        host_config.normalize_paths();
        self.default_workdir = host_config.default_workdir;
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        HostRuntimeConfig::from(self)
            .into_normalized_validated()
            .map(|_| ())?;
        self.validate_http_auth()?;
        anyhow::ensure!(
            self.request_timeout_ms > 0,
            "request_timeout_ms must be greater than zero"
        );
        match self.connection_mode {
            DaemonConnectionMode::Listen => anyhow::ensure!(
                self.reverse.is_none(),
                "reverse config requires connection_mode = \"reverse\""
            ),
            DaemonConnectionMode::Reverse => {
                let reverse = self
                    .reverse
                    .as_ref()
                    .context("reverse config is required when connection_mode = \"reverse\"")?;
                anyhow::ensure!(
                    reverse.min_idle_connections > 0,
                    "reverse.min_idle_connections must be greater than zero"
                );
                anyhow::ensure!(
                    reverse.min_idle_connections <= reverse.max_connections,
                    "reverse.min_idle_connections must not exceed reverse.max_connections"
                );
                anyhow::ensure!(
                    reverse.reconnect_min_ms > 0
                        && reverse.reconnect_min_ms <= reverse.reconnect_max_ms,
                    "reverse reconnect bounds are invalid"
                );
                anyhow::ensure!(
                    reverse.registration_timeout_ms > 0,
                    "reverse.registration_timeout_ms must be greater than zero"
                );
                match reverse.transport {
                    DaemonTransport::Tls => anyhow::ensure!(
                        reverse.tls.is_some(),
                        "reverse.tls is required for TLS reverse transport"
                    ),
                    DaemonTransport::Http => {
                        anyhow::ensure!(
                            reverse.allow_insecure_http,
                            "reverse HTTP requires reverse.allow_insecure_http = true"
                        );
                        anyhow::ensure!(
                            reverse.bearer_token.is_some(),
                            "reverse HTTP requires reverse.bearer_token"
                        );
                    }
                }
                if reverse
                    .tls
                    .as_ref()
                    .is_some_and(|tls| tls.pinned_server_cert_pem.is_some())
                {
                    anyhow::bail!(
                        "reverse.tls.pinned_server_cert_pem is not supported; use CA and server_name verification"
                    );
                }
            }
        }
        crate::tls::validate_config(self)?;
        Ok(())
    }

    pub fn request_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.request_timeout_ms)
    }

    pub fn into_validated(mut self) -> anyhow::Result<ValidatedDaemonConfig> {
        self.normalize_paths();
        self.validate()?;
        Ok(ValidatedDaemonConfig(self))
    }

    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<ValidatedDaemonConfig> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        let config: Self = toml::from_str(&text)?;
        config.into_validated()
    }
}

fn default_reverse_min_idle_connections() -> usize {
    4
}

fn default_daemon_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

fn default_reverse_max_connections() -> usize {
    128
}

fn default_reverse_reconnect_min_ms() -> u64 {
    250
}

fn default_reverse_reconnect_max_ms() -> u64 {
    10_000
}

fn default_reverse_registration_timeout_ms() -> u64 {
    5_000
}

impl ValidatedDaemonConfig {
    pub fn into_inner(self) -> DaemonConfig {
        self.0
    }
}

impl AsRef<DaemonConfig> for ValidatedDaemonConfig {
    fn as_ref(&self) -> &DaemonConfig {
        &self.0
    }
}

impl Deref for ValidatedDaemonConfig {
    type Target = DaemonConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<DaemonConfig> for HostRuntimeConfig {
    fn from(value: DaemonConfig) -> Self {
        Self::from(&value)
    }
}

impl From<&DaemonConfig> for HostRuntimeConfig {
    fn from(value: &DaemonConfig) -> Self {
        Self::from_source(value.host_runtime_config_source())
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

fn default_request_timeout_ms() -> u64 {
    300_000
}
