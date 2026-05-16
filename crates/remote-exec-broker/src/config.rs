use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS;
use remote_exec_host::{
    EmbeddedHostConfig, HostPortForwardLimits, ProcessEnvironment, PtyMode, YieldTimeConfig,
};
pub use remote_exec_proto::auth::HttpAuthConfig;
use remote_exec_proto::sandbox::FilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;
use serde::Deserialize;

use crate::port_forward::BrokerPortForwardLimits;
use crate::state::LOCAL_TARGET_NAME;

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
    pub port_forward_limits: BrokerPortForwardLimits,
}

#[derive(Debug, Clone)]
pub struct ValidatedBrokerConfig(BrokerConfig);

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
    pub(crate) fn validate(&self, name: &str) -> anyhow::Result<()> {
        self.timeouts.validate(name)?;

        if let Some(http_auth) = &self.http_auth {
            http_auth.validate(&format!("target `{name}`"))?;
        }

        if self.base_url.starts_with("http://") {
            return self.validate_http_transport(name);
        }

        self.validate_https_transport(name)
    }

    pub(crate) fn transport_kind(&self, name: &str) -> anyhow::Result<TargetTransportKind> {
        self.validate(name)?;
        Ok(if self.base_url.starts_with("http://") {
            TargetTransportKind::Http
        } else {
            TargetTransportKind::Https
        })
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
    fn normalized_default_workdir(&self) -> PathBuf {
        remote_exec_host::config::normalize_configured_workdir(
            &self.default_workdir,
            self.windows_posix_root.as_deref(),
        )
    }

    pub fn embedded_host_config(
        &self,
        sandbox: Option<FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> EmbeddedHostConfig {
        EmbeddedHostConfig {
            target: LOCAL_TARGET_NAME.to_string(),
            default_workdir: self.default_workdir.clone(),
            windows_posix_root: self.windows_posix_root.clone(),
            sandbox,
            enable_transfer_compression,
            transfer_limits: self.transfer_limits,
            max_open_sessions: DEFAULT_MAX_OPEN_SESSIONS,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.clone(),
            yield_time: self.yield_time,
            port_forward_limits: self.port_forward_limits,
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
    pub(crate) fn normalize_paths(&mut self) {
        if let Some(local) = &mut self.local {
            local.default_workdir = local.normalized_default_workdir();
        }
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        self.mcp.validate()?;
        self.transfer_limits.validate()?;
        self.port_forward_limits.validate()?;
        anyhow::ensure!(
            !self.targets.contains_key(LOCAL_TARGET_NAME),
            "configured target name `{LOCAL_TARGET_NAME}` is reserved for broker-host filesystem access"
        );
        if let Some(local) = &self.local {
            local.transfer_limits.validate()?;
            remote_exec_host::config::validate_existing_directory(
                &local.default_workdir,
                "local.default_workdir",
            )?;
        }
        for (name, target) in &self.targets {
            target.validate(name)?;
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

fn default_target_connect_timeout_ms() -> u64 {
    DEFAULT_TARGET_CONNECT_TIMEOUT_MS
}

fn default_target_read_timeout_ms() -> u64 {
    DEFAULT_TARGET_READ_TIMEOUT_MS
}

fn default_target_request_timeout_ms() -> u64 {
    DEFAULT_TARGET_REQUEST_TIMEOUT_MS
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
mod tests {
    #[cfg(windows)]
    use std::path::PathBuf;

    use crate::state::LOCAL_TARGET_NAME;

    use super::{BrokerConfig, McpServerConfig, SseInterval};

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
        tokio::fs::write(&config_path, valid_target_config(LOCAL_TARGET_NAME))
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

    #[tokio::test]
    async fn load_accepts_remote_target_timeout_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-a]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-a.timeouts]
connect_ms = 1234
read_ms = 2345
request_ms = 3456
startup_probe_ms = 4567
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        let timeouts = config.targets["builder-a"].timeouts;
        assert_eq!(timeouts.connect_ms, 1234);
        assert_eq!(timeouts.read_ms, 2345);
        assert_eq!(timeouts.request_ms, 3456);
        assert_eq!(timeouts.startup_probe_ms, 4567);
        assert_eq!(
            timeouts.request_timeout(),
            std::time::Duration::from_millis(3456)
        );
    }

    #[tokio::test]
    async fn load_rejects_zero_remote_target_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-a]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-a.timeouts]
request_ms = 0
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("target `builder-a` timeouts.request_ms must be greater than zero"),
            "unexpected error: {err}"
        );
    }

    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn load_accepts_https_targets_even_when_broker_tls_feature_disabled() {
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
        assert_eq!(
            config.targets["builder-a"].base_url,
            "https://127.0.0.1:8443"
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

    #[tokio::test]
    async fn load_rejects_missing_local_default_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        let missing_workdir = dir.path().join("missing-local-workdir");
        tokio::fs::write(
            &config_path,
            format!(
                "[local]\ndefault_workdir = {}\n",
                toml::Value::String(missing_workdir.display().to_string())
            ),
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string().contains("local.default_workdir")
                && err.to_string().contains("does not exist"),
            "unexpected error: {err}"
        );
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

    #[cfg(windows)]
    #[tokio::test]
    async fn load_normalizes_local_default_workdir_through_windows_posix_root() {
        let dir = tempfile::tempdir().unwrap();
        let synthetic_root = dir.path().join("msys64");
        std::fs::create_dir_all(synthetic_root.join("tmp")).unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            format!(
                "[local]\ndefault_workdir = \"/tmp\"\nwindows_posix_root = {}\n",
                toml::Value::String(synthetic_root.display().to_string())
            ),
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config
                .local
                .as_ref()
                .map(|local| local.default_workdir.clone()),
            Some(synthetic_root.join("tmp"))
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
    async fn bundled_broker_example_preserves_intentional_structured_content_override() {
        let example_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../configs/broker.example.toml");
        let example_text = tokio::fs::read_to_string(&example_path).await.unwrap();

        let config: BrokerConfig = toml::from_str(&example_text).unwrap();
        config.validate().unwrap();
        assert!(config.disable_structured_content);
        assert!(matches!(config.mcp, McpServerConfig::Stdio));
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
    async fn load_accepts_http_bearer_auth_for_target() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true
expected_daemon_name = "builder-xp"

[targets.builder-xp.http_auth]
bearer_token = "shared-secret"
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config.targets["builder-xp"]
                .http_auth
                .as_ref()
                .map(|auth| auth.bearer_token.as_str()),
            Some("shared-secret")
        );
    }

    #[tokio::test]
    async fn load_rejects_empty_http_bearer_auth_for_target() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-xp]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-xp.http_auth]
bearer_token = ""
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("http_auth.bearer_token must not be empty"),
            "unexpected error: {err}"
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
        match &config.mcp {
            McpServerConfig::StreamableHttp {
                listen,
                path,
                stateful,
                sse_keep_alive,
                sse_retry,
            } => {
                assert_eq!(*listen, "127.0.0.1:8787".parse().unwrap());
                assert_eq!(path, "/rpc");
                assert!(!stateful);
                assert_eq!(*sse_keep_alive, SseInterval::Disabled);
                assert_eq!(
                    *sse_retry,
                    SseInterval::Duration(std::time::Duration::from_millis(1000))
                );
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
