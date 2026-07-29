use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use remote_exec_host::config::{
    DEFAULT_MAX_OPEN_SESSIONS, HostRuntimeConfigSource, HostRuntimeConfigValidation,
};
use remote_exec_host::{
    HostPortForwardLimits, HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig,
};
pub use remote_exec_proto::auth::HttpAuthConfig;
use remote_exec_proto::sandbox::FilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;
use serde::Deserialize;

use crate::local;
use crate::port_forward::BrokerPortForwardLimits;

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
    pub transfer_limits: TransferLimits,
    #[serde(default)]
    pub disable_structured_content: bool,
    #[serde(default)]
    pub tools: BrokerToolsConfig,
    #[serde(default)]
    pub port_forward_limits: BrokerPortForwardLimits,
    #[serde(default)]
    pub health_refresh: BrokerHealthRefreshConfig,
    #[serde(default)]
    pub reverse: Option<ReverseListenerConfig>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReverseTransport {
    #[default]
    Tls,
    Http,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReverseListenerConfig {
    pub listen: SocketAddr,
    #[serde(default)]
    pub transport: ReverseTransport,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub tls: Option<ReverseListenerTlsConfig>,
    #[serde(default = "default_reverse_registration_timeout_ms")]
    pub registration_timeout_ms: u64,
    #[serde(default = "default_reverse_lane_wait_timeout_ms")]
    pub lane_wait_timeout_ms: u64,
    #[serde(default = "default_reverse_max_connections")]
    pub max_connections: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReverseListenerTlsConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub ca_pem: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ValidatedBrokerConfig(BrokerConfig);

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub struct BrokerToolsConfig {
    #[serde(default)]
    pub file: FileToolConfig,
}

const DEFAULT_TARGET_HEALTHY_REFRESH_INTERVAL_MS: u64 = 60_000;
const DEFAULT_TARGET_UNHEALTHY_REFRESH_INTERVAL_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrokerHealthRefreshConfig {
    pub healthy_interval_ms: u64,
    pub unhealthy_interval_ms: u64,
}

impl Default for BrokerHealthRefreshConfig {
    fn default() -> Self {
        Self {
            healthy_interval_ms: default_target_healthy_health_refresh_interval_ms(),
            unhealthy_interval_ms: default_target_unhealthy_health_refresh_interval_ms(),
        }
    }
}

impl<'de> Deserialize<'de> for BrokerHealthRefreshConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawBrokerHealthRefreshConfig {
            #[serde(default)]
            healthy_interval_ms: Option<u64>,
            #[serde(default)]
            unhealthy_interval_ms: Option<u64>,
            #[serde(default)]
            interval_ms: Option<u64>,
        }

        let raw = RawBrokerHealthRefreshConfig::deserialize(deserializer)?;
        Ok(Self {
            healthy_interval_ms: raw
                .healthy_interval_ms
                .or(raw.interval_ms)
                .unwrap_or_else(default_target_healthy_health_refresh_interval_ms),
            unhealthy_interval_ms: raw
                .unhealthy_interval_ms
                .or(raw.interval_ms)
                .unwrap_or_else(default_target_unhealthy_health_refresh_interval_ms),
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct FileToolConfig {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub edit: bool,
    #[serde(default = "default_file_tool_read_limit_lines")]
    pub default_read_limit_lines: u64,
    #[serde(default = "default_file_tool_max_read_limit_lines")]
    pub max_read_limit_lines: u64,
    #[serde(default = "default_file_tool_max_read_bytes")]
    pub max_read_bytes: u64,
}

impl Default for FileToolConfig {
    fn default() -> Self {
        Self {
            read: false,
            write: false,
            edit: false,
            default_read_limit_lines: default_file_tool_read_limit_lines(),
            max_read_limit_lines: default_file_tool_max_read_limit_lines(),
            max_read_bytes: default_file_tool_max_read_bytes(),
        }
    }
}

impl FileToolConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.default_read_limit_lines > 0,
            "tools.file.default_read_limit_lines must be greater than zero"
        );
        anyhow::ensure!(
            self.max_read_limit_lines > 0,
            "tools.file.max_read_limit_lines must be greater than zero"
        );
        anyhow::ensure!(
            self.default_read_limit_lines <= self.max_read_limit_lines,
            "tools.file.default_read_limit_lines must be less than or equal to tools.file.max_read_limit_lines"
        );
        anyhow::ensure!(
            self.max_read_bytes > 0,
            "tools.file.max_read_bytes must be greater than zero"
        );
        Ok(())
    }
}

impl BrokerToolsConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        self.file.validate()
    }
}

impl BrokerHealthRefreshConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.healthy_interval_ms > 0,
            "health_refresh.healthy_interval_ms must be greater than zero"
        );
        anyhow::ensure!(
            self.unhealthy_interval_ms > 0,
            "health_refresh.unhealthy_interval_ms must be greater than zero"
        );
        Ok(())
    }

    pub(crate) fn healthy_interval(self) -> Duration {
        Duration::from_millis(self.healthy_interval_ms)
    }

    pub(crate) fn unhealthy_interval(self) -> Duration {
        Duration::from_millis(self.unhealthy_interval_ms)
    }
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
        #[serde(
            default = "default_streamable_http_sse_keep_alive",
            rename = "sse_keep_alive_ms",
            deserialize_with = "deserialize_sse_interval"
        )]
        sse_keep_alive: SseInterval,
        #[serde(
            default = "default_streamable_http_sse_retry",
            rename = "sse_retry_ms",
            deserialize_with = "deserialize_sse_interval"
        )]
        sse_retry: SseInterval,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseInterval {
    Disabled,
    Duration(std::time::Duration),
}

impl SseInterval {
    pub(crate) fn as_duration(self) -> Option<std::time::Duration> {
        match self {
            Self::Disabled => None,
            Self::Duration(duration) => Some(duration),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
    pub base_url: String,
    #[serde(default)]
    pub http_auth: Option<HttpAuthConfig>,
    #[serde(default)]
    pub timeouts: TargetTimeoutConfig,
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

const DEFAULT_TARGET_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_TARGET_READ_TIMEOUT_MS: u64 = 310_000;
const DEFAULT_TARGET_REQUEST_TIMEOUT_MS: u64 = 310_000;
const DEFAULT_TARGET_STARTUP_PROBE_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct TargetTimeoutConfig {
    #[serde(default = "default_target_connect_timeout_ms")]
    pub connect_ms: u64,
    #[serde(default = "default_target_read_timeout_ms")]
    pub read_ms: u64,
    #[serde(default = "default_target_request_timeout_ms")]
    pub request_ms: u64,
    #[serde(default = "default_target_startup_probe_timeout_ms")]
    pub startup_probe_ms: u64,
}

impl Default for TargetTimeoutConfig {
    fn default() -> Self {
        Self {
            connect_ms: DEFAULT_TARGET_CONNECT_TIMEOUT_MS,
            read_ms: DEFAULT_TARGET_READ_TIMEOUT_MS,
            request_ms: DEFAULT_TARGET_REQUEST_TIMEOUT_MS,
            startup_probe_ms: DEFAULT_TARGET_STARTUP_PROBE_TIMEOUT_MS,
        }
    }
}

impl TargetTimeoutConfig {
    pub(crate) fn validate(&self, target_name: &str) -> anyhow::Result<()> {
        validate_timeout_ms(target_name, "connect_ms", self.connect_ms)?;
        validate_timeout_ms(target_name, "read_ms", self.read_ms)?;
        validate_timeout_ms(target_name, "request_ms", self.request_ms)?;
        validate_timeout_ms(target_name, "startup_probe_ms", self.startup_probe_ms)?;
        Ok(())
    }

    pub(crate) fn connect_timeout(self) -> Duration {
        Duration::from_millis(self.connect_ms)
    }

    pub(crate) fn read_timeout(self) -> Duration {
        Duration::from_millis(self.read_ms)
    }

    pub(crate) fn request_timeout(self) -> Duration {
        Duration::from_millis(self.request_ms)
    }

    pub(crate) fn startup_probe_timeout(self) -> Duration {
        Duration::from_millis(self.startup_probe_ms)
    }
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
    pub transfer_limits: TransferLimits,
    #[serde(default)]
    pub port_forward_limits: HostPortForwardLimits,
    #[serde(default)]
    pub experimental_apply_patch_target_encoding_autodetect: bool,
}

impl TargetConfig {
    pub(crate) fn is_reverse(&self) -> bool {
        self.base_url == "reverse://"
    }

    pub(crate) fn validate(&self, name: &str) -> anyhow::Result<()> {
        self.timeouts.validate(name)?;

        if let Some(http_auth) = &self.http_auth {
            http_auth.validate(&format!("target `{name}`"))?;
        }

        if self.is_reverse() {
            anyhow::ensure!(
                !self.skip_server_name_verification,
                "target `{name}` cannot set skip_server_name_verification in reverse mode"
            );
            anyhow::ensure!(
                self.pinned_server_cert_pem.is_none(),
                "target `{name}` cannot set pinned_server_cert_pem in reverse mode; reverse TLS identity is bound by the client certificate common name"
            );
            return Ok(());
        }

        if self.base_url.starts_with("http://") {
            return self.validate_http_transport(name);
        }

        self.validate_https_transport(name)
    }

    pub(crate) fn transport_kind(&self) -> TargetTransportKind {
        if self.base_url.starts_with("http://") {
            TargetTransportKind::Http
        } else {
            TargetTransportKind::Https
        }
    }

    fn validate_http_transport(&self, name: &str) -> anyhow::Result<()> {
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
        Ok(())
    }

    fn validate_https_transport(&self, name: &str) -> anyhow::Result<()> {
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
    fn host_runtime_config_source<'a>(
        &'a self,
        sandbox: Option<&'a FilesystemSandbox>,
        enable_transfer_compression: bool,
        process_environment: &'a ProcessEnvironment,
    ) -> HostRuntimeConfigSource<'a> {
        HostRuntimeConfigSource {
            target: local::TARGET_NAME,
            default_workdir: &self.default_workdir,
            windows_posix_root: self.windows_posix_root.as_deref(),
            sandbox,
            enable_transfer_compression,
            transfer_limits: self.transfer_limits,
            max_open_sessions: DEFAULT_MAX_OPEN_SESSIONS,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.as_deref(),
            yield_time: self.yield_time,
            port_forward_limits: self.port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment,
        }
    }

    pub fn host_runtime_config(
        &self,
        sandbox: Option<FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> HostRuntimeConfig {
        let process_environment = ProcessEnvironment::capture_current();
        HostRuntimeConfig::from_source(self.host_runtime_config_source(
            sandbox.as_ref(),
            enable_transfer_compression,
            &process_environment,
        ))
    }

    fn validate_host_runtime_config(
        &self,
        sandbox: Option<&FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> anyhow::Result<HostRuntimeConfig> {
        let process_environment = ProcessEnvironment::capture_current();
        self.host_runtime_config_source(sandbox, enable_transfer_compression, &process_environment)
            .into_normalized_validated_config(HostRuntimeConfigValidation::new(
                "local.default_workdir",
            ))
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
    pub(crate) fn normalize_paths(&mut self) {
        if let Some(local) = &mut self.local {
            let process_environment = ProcessEnvironment::capture_current();
            let mut host_config = HostRuntimeConfig::from_source(local.host_runtime_config_source(
                None,
                self.enable_transfer_compression,
                &process_environment,
            ));
            host_config.normalize_paths();
            local.default_workdir = host_config.default_workdir;
        }
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        self.mcp.validate()?;
        self.transfer_limits.validate()?;
        self.port_forward_limits.validate()?;
        self.tools.validate()?;
        self.health_refresh.validate()?;
        if let Some(reverse) = &self.reverse {
            anyhow::ensure!(
                reverse.registration_timeout_ms > 0,
                "reverse.registration_timeout_ms must be greater than zero"
            );
            anyhow::ensure!(
                reverse.lane_wait_timeout_ms > 0,
                "reverse.lane_wait_timeout_ms must be greater than zero"
            );
            anyhow::ensure!(
                reverse.max_connections > 0,
                "reverse.max_connections must be greater than zero"
            );
            match reverse.transport {
                ReverseTransport::Tls => anyhow::ensure!(
                    reverse.tls.is_some(),
                    "reverse.tls is required when reverse.transport = \"tls\""
                ),
                ReverseTransport::Http => {
                    anyhow::ensure!(
                        reverse.allow_insecure_http,
                        "reverse plain HTTP requires reverse.allow_insecure_http = true"
                    );
                    anyhow::ensure!(
                        reverse.tls.is_none(),
                        "reverse.tls cannot be set when reverse.transport = \"http\""
                    );
                }
            }
        }
        anyhow::ensure!(
            !self.targets.contains_key(local::TARGET_NAME),
            "configured target name `{}` is reserved for broker-host filesystem access",
            local::TARGET_NAME
        );
        if let Some(local) = &self.local {
            local
                .validate_host_runtime_config(
                    self.host_sandbox.as_ref(),
                    self.enable_transfer_compression,
                )
                .map(|_| ())?;
        }
        for (name, target) in &self.targets {
            target.validate(name)?;
            if target.is_reverse() {
                anyhow::ensure!(
                    self.reverse.is_some(),
                    "target `{name}` uses reverse mode but broker reverse listener is not configured"
                );
                if self
                    .reverse
                    .as_ref()
                    .is_some_and(|reverse| reverse.transport == ReverseTransport::Http)
                {
                    anyhow::ensure!(
                        target.http_auth.is_some(),
                        "reverse HTTP target `{name}` requires http_auth for lane registration"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn into_validated(mut self) -> anyhow::Result<ValidatedBrokerConfig> {
        self.normalize_paths();
        self.validate()?;
        Ok(ValidatedBrokerConfig(self))
    }

    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<ValidatedBrokerConfig> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        let config: Self = toml::from_str(&text)?;
        config.into_validated()
    }
}

impl ValidatedBrokerConfig {
    pub fn into_inner(self) -> BrokerConfig {
        self.0
    }
}

impl AsRef<BrokerConfig> for ValidatedBrokerConfig {
    fn as_ref(&self) -> &BrokerConfig {
        &self.0
    }
}

impl Deref for ValidatedBrokerConfig {
    type Target = BrokerConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn default_allow_login_shell() -> bool {
    true
}

fn default_enable_transfer_compression() -> bool {
    true
}

fn default_reverse_registration_timeout_ms() -> u64 {
    5_000
}

fn default_reverse_lane_wait_timeout_ms() -> u64 {
    30_000
}

fn default_reverse_max_connections() -> usize {
    1024
}

fn default_file_tool_read_limit_lines() -> u64 {
    2_000
}

fn default_file_tool_max_read_limit_lines() -> u64 {
    20_000
}

fn default_file_tool_max_read_bytes() -> u64 {
    4 * 1024 * 1024
}

fn default_target_connect_timeout_ms() -> u64 {
    DEFAULT_TARGET_CONNECT_TIMEOUT_MS
}

fn default_target_read_timeout_ms() -> u64 {
    DEFAULT_TARGET_READ_TIMEOUT_MS
}

fn default_target_request_timeout_ms() -> u64 {
    DEFAULT_TARGET_REQUEST_TIMEOUT_MS
}

fn default_target_healthy_health_refresh_interval_ms() -> u64 {
    DEFAULT_TARGET_HEALTHY_REFRESH_INTERVAL_MS
}

fn default_target_unhealthy_health_refresh_interval_ms() -> u64 {
    DEFAULT_TARGET_UNHEALTHY_REFRESH_INTERVAL_MS
}

fn default_target_startup_probe_timeout_ms() -> u64 {
    DEFAULT_TARGET_STARTUP_PROBE_TIMEOUT_MS
}

fn validate_timeout_ms(target_name: &str, field: &str, value: u64) -> anyhow::Result<()> {
    anyhow::ensure!(
        value > 0,
        "target `{target_name}` timeouts.{field} must be greater than zero"
    );
    Ok(())
}

fn default_streamable_http_path() -> String {
    "/mcp".to_string()
}

fn default_streamable_http_stateful() -> bool {
    true
}

fn deserialize_sse_interval<'de, D>(deserializer: D) -> Result<SseInterval, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let millis = u64::deserialize(deserializer)?;
    Ok(if millis == 0 {
        SseInterval::Disabled
    } else {
        SseInterval::Duration(std::time::Duration::from_millis(millis))
    })
}

fn default_streamable_http_sse_keep_alive() -> SseInterval {
    SseInterval::Duration(std::time::Duration::from_millis(15_000))
}

fn default_streamable_http_sse_retry() -> SseInterval {
    SseInterval::Duration(std::time::Duration::from_millis(3_000))
}

#[cfg(test)]
mod tests;
