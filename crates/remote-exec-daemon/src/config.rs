use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use remote_exec_proto::sandbox::FilesystemSandbox;
use serde::{Deserialize, Deserializer};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YieldTimeOperation {
    ExecCommand,
    WriteStdinPoll,
    WriteStdinInput,
}

impl YieldTimeOperation {
    fn config_path(self) -> &'static str {
        match self {
            Self::ExecCommand => "yield_time.exec_command",
            Self::WriteStdinPoll => "yield_time.write_stdin_poll",
            Self::WriteStdinInput => "yield_time.write_stdin_input",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YieldTimeOperationConfig {
    pub default_ms: u64,
    pub max_ms: u64,
    pub min_ms: u64,
}

impl YieldTimeOperationConfig {
    pub const fn new(default_ms: u64, max_ms: u64, min_ms: u64) -> Self {
        Self {
            default_ms,
            max_ms,
            min_ms,
        }
    }

    pub fn resolve_ms(self, requested_ms: Option<u64>) -> u64 {
        requested_ms
            .unwrap_or(self.default_ms)
            .clamp(self.min_ms, self.max_ms)
    }

    fn validate(self, operation: YieldTimeOperation) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.min_ms <= self.max_ms,
            "{}.min_ms must be less than or equal to {}.max_ms",
            operation.config_path(),
            operation.config_path()
        );
        anyhow::ensure!(
            self.default_ms >= self.min_ms && self.default_ms <= self.max_ms,
            "{}.default_ms must be between {}.min_ms and {}.max_ms",
            operation.config_path(),
            operation.config_path(),
            operation.config_path()
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YieldTimeConfig {
    pub exec_command: YieldTimeOperationConfig,
    pub write_stdin_poll: YieldTimeOperationConfig,
    pub write_stdin_input: YieldTimeOperationConfig,
}

impl YieldTimeConfig {
    pub fn resolve_ms(self, operation: YieldTimeOperation, requested_ms: Option<u64>) -> u64 {
        self.operation_config(operation).resolve_ms(requested_ms)
    }

    fn operation_config(self, operation: YieldTimeOperation) -> YieldTimeOperationConfig {
        match operation {
            YieldTimeOperation::ExecCommand => self.exec_command,
            YieldTimeOperation::WriteStdinPoll => self.write_stdin_poll,
            YieldTimeOperation::WriteStdinInput => self.write_stdin_input,
        }
    }

    fn validate(self) -> anyhow::Result<()> {
        self.exec_command
            .validate(YieldTimeOperation::ExecCommand)?;
        self.write_stdin_poll
            .validate(YieldTimeOperation::WriteStdinPoll)?;
        self.write_stdin_input
            .validate(YieldTimeOperation::WriteStdinInput)?;
        Ok(())
    }
}

impl Default for YieldTimeConfig {
    fn default() -> Self {
        Self {
            exec_command: default_exec_command_yield_time(),
            write_stdin_poll: default_write_stdin_poll_yield_time(),
            write_stdin_input: default_write_stdin_input_yield_time(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
struct YieldTimeConfigOverride {
    #[serde(default)]
    exec_command: YieldTimeOperationConfigOverride,
    #[serde(default)]
    write_stdin_poll: YieldTimeOperationConfigOverride,
    #[serde(default)]
    write_stdin_input: YieldTimeOperationConfigOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
struct YieldTimeOperationConfigOverride {
    #[serde(default)]
    default_ms: Option<u64>,
    #[serde(default)]
    max_ms: Option<u64>,
    #[serde(default)]
    min_ms: Option<u64>,
}

impl YieldTimeConfigOverride {
    fn resolve(self) -> YieldTimeConfig {
        YieldTimeConfig {
            exec_command: self.exec_command.resolve(default_exec_command_yield_time()),
            write_stdin_poll: self
                .write_stdin_poll
                .resolve(default_write_stdin_poll_yield_time()),
            write_stdin_input: self
                .write_stdin_input
                .resolve(default_write_stdin_input_yield_time()),
        }
    }
}

impl YieldTimeOperationConfigOverride {
    fn resolve(self, defaults: YieldTimeOperationConfig) -> YieldTimeOperationConfig {
        YieldTimeOperationConfig {
            default_ms: self.default_ms.unwrap_or(defaults.default_ms),
            max_ms: self.max_ms.unwrap_or(defaults.max_ms),
            min_ms: self.min_ms.unwrap_or(defaults.min_ms),
        }
    }
}

impl<'de> Deserialize<'de> for YieldTimeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(YieldTimeConfigOverride::deserialize(deserializer)?.resolve())
    }
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
    pub windows_posix_root: Option<PathBuf>,
    #[serde(default)]
    pub transport: DaemonTransport,
    #[serde(default)]
    pub http_auth: Option<HttpAuthConfig>,
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
    #[serde(default)]
    pub yield_time: YieldTimeConfig,
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
    pub windows_posix_root: Option<PathBuf>,
    pub sandbox: Option<FilesystemSandbox>,
    pub enable_transfer_compression: bool,
    pub allow_login_shell: bool,
    pub pty: PtyMode,
    pub default_shell: Option<String>,
    pub yield_time: YieldTimeConfig,
    pub experimental_apply_patch_target_encoding_autodetect: bool,
    pub process_environment: ProcessEnvironment,
}

impl EmbeddedDaemonConfig {
    pub fn into_daemon_config(self) -> DaemonConfig {
        DaemonConfig {
            target: self.target,
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            default_workdir: self.default_workdir,
            windows_posix_root: self.windows_posix_root,
            transport: DaemonTransport::Http,
            http_auth: None,
            sandbox: self.sandbox,
            enable_transfer_compression: self.enable_transfer_compression,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell,
            yield_time: self.yield_time,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: self.process_environment,
            tls: None,
        }
    }
}

impl DaemonConfig {
    fn normalized_default_workdir(&self) -> PathBuf {
        normalize_configured_workdir(&self.default_workdir, self.windows_posix_root.as_deref())
    }

    fn validate_windows_posix_root(&self) -> anyhow::Result<()> {
        #[cfg(windows)]
        if let Some(root) = &self.windows_posix_root {
            anyhow::ensure!(
                root.is_absolute(),
                "windows_posix_root must be an absolute path"
            );
        }

        Ok(())
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

    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_windows_posix_root()?;
        validate_existing_directory(&self.normalized_default_workdir(), "default_workdir")?;
        self.validate_http_auth()?;
        self.yield_time.validate()?;
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

pub fn normalize_configured_workdir(path: &Path, windows_posix_root: Option<&Path>) -> PathBuf {
    crate::host_path::resolve_absolute_input_path(&path.to_string_lossy(), windows_posix_root)
        .unwrap_or_else(|| path.to_path_buf())
}

fn validate_existing_directory(path: &Path, field_name: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("{field_name} `{}` does not exist", path.display()))?;
    anyhow::ensure!(
        metadata.is_dir(),
        "{field_name} `{}` must be a directory",
        path.display()
    );
    Ok(())
}

impl HttpAuthConfig {
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

const fn default_exec_command_yield_time() -> YieldTimeOperationConfig {
    YieldTimeOperationConfig::new(10_000, 30_000, 250)
}

const fn default_write_stdin_poll_yield_time() -> YieldTimeOperationConfig {
    YieldTimeOperationConfig::new(5_000, 300_000, 5_000)
}

const fn default_write_stdin_input_yield_time() -> YieldTimeOperationConfig {
    YieldTimeOperationConfig::new(250, 30_000, 250)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DaemonConfig, DaemonTransport, YieldTimeConfig, YieldTimeOperation};

    fn neutral_toml_path(path: &Path) -> toml::Value {
        toml::Value::String(path.display().to_string())
    }

    fn neutral_workdir(dir: &tempfile::TempDir) -> toml::Value {
        neutral_toml_path(dir.path())
    }

    #[tokio::test]
    async fn load_accepts_http_transport_without_tls_block() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert!(matches!(config.transport, DaemonTransport::Http));
        assert!(config.http_auth.is_none());
        assert!(config.tls.is_none());
        assert_eq!(config.yield_time, YieldTimeConfig::default());
        assert!(!config.experimental_apply_patch_target_encoding_autodetect);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn load_accepts_windows_posix_root() {
        let dir = tempfile::tempdir().unwrap();
        let synthetic_root = dir.path().join("msys64");
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
windows_posix_root = {}
transport = "http"
"#,
                neutral_workdir(&dir),
                neutral_toml_path(&synthetic_root)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert_eq!(config.windows_posix_root, Some(synthetic_root));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn load_normalizes_default_workdir_through_windows_posix_root() {
        let dir = tempfile::tempdir().unwrap();
        let synthetic_root = dir.path().join("msys64");
        let posix_workdir_name = "tmp";
        let posix_workdir = format!("/{posix_workdir_name}");
        std::fs::create_dir_all(synthetic_root.join(posix_workdir_name)).unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = "{posix_workdir}"
windows_posix_root = {}
transport = "http"
"#,
                neutral_toml_path(&synthetic_root)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config.default_workdir,
            synthetic_root.join(posix_workdir_name)
        );
    }

    #[tokio::test]
    async fn load_rejects_default_tls_transport_without_tls_block() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        if cfg!(feature = "tls") {
            assert!(
                err.to_string()
                    .contains("tls config is required when transport = \"tls\""),
                "unexpected error: {err}"
            );
        } else {
            assert!(
                err.to_string()
                    .contains(crate::tls::FEATURE_REQUIRED_MESSAGE),
                "unexpected error: {err}"
            );
        }
    }

    #[tokio::test]
    async fn load_rejects_missing_default_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        let missing_workdir = dir.path().join("missing-workdir");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"
"#,
                toml::Value::String(missing_workdir.display().to_string())
            ),
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string().contains("default_workdir")
                && err.to_string().contains("does not exist"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn yield_time_defaults_preserve_existing_behavior() {
        let config = YieldTimeConfig::default();

        assert_eq!(
            config.resolve_ms(YieldTimeOperation::ExecCommand, None),
            10_000
        );
        assert_eq!(
            config.resolve_ms(YieldTimeOperation::ExecCommand, Some(1)),
            250
        );
        assert_eq!(
            config.resolve_ms(YieldTimeOperation::WriteStdinPoll, None),
            5_000
        );
        assert_eq!(
            config.resolve_ms(YieldTimeOperation::WriteStdinPoll, Some(1)),
            5_000
        );
        assert_eq!(
            config.resolve_ms(YieldTimeOperation::WriteStdinPoll, Some(400_000)),
            300_000
        );
        assert_eq!(
            config.resolve_ms(YieldTimeOperation::WriteStdinInput, None),
            250
        );
        assert_eq!(
            config.resolve_ms(YieldTimeOperation::WriteStdinInput, Some(100_000)),
            30_000
        );
    }

    #[tokio::test]
    async fn load_merges_partial_yield_time_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[yield_time.exec_command]
max_ms = 60000

[yield_time.write_stdin_poll]
default_ms = 12000
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert_eq!(config.yield_time.exec_command.default_ms, 10_000);
        assert_eq!(config.yield_time.exec_command.min_ms, 250);
        assert_eq!(config.yield_time.exec_command.max_ms, 60_000);
        assert_eq!(config.yield_time.write_stdin_poll.default_ms, 12_000);
        assert_eq!(config.yield_time.write_stdin_poll.min_ms, 5_000);
        assert_eq!(config.yield_time.write_stdin_poll.max_ms, 300_000);
        assert_eq!(config.yield_time.write_stdin_input.default_ms, 250);
    }

    #[tokio::test]
    async fn load_rejects_invalid_yield_time_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[yield_time.exec_command]
default_ms = 100
min_ms = 200
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("yield_time.exec_command.default_ms must be between"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn load_accepts_tls_transport_with_pinned_client_cert() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        let daemon_cert = dir.path().join("daemon.pem");
        let daemon_key = dir.path().join("daemon.key");
        let ca_cert = dir.path().join("ca.pem");
        let broker_cert = dir.path().join("broker.pem");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}

[tls]
cert_pem = {}
key_pem = {}
ca_pem = {}
pinned_client_cert_pem = {}
"#,
                neutral_workdir(&dir),
                neutral_toml_path(&daemon_cert),
                neutral_toml_path(&daemon_key),
                neutral_toml_path(&ca_cert),
                neutral_toml_path(&broker_cert)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config
                .tls
                .as_ref()
                .and_then(|tls| tls.pinned_client_cert_pem.as_ref()),
            Some(&broker_cert)
        );
    }

    #[tokio::test]
    async fn load_accepts_experimental_apply_patch_target_encoding_autodetect() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}
transport = "http"
experimental_apply_patch_target_encoding_autodetect = true
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert!(config.experimental_apply_patch_target_encoding_autodetect);
    }

    #[tokio::test]
    async fn load_accepts_http_bearer_auth() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[http_auth]
bearer_token = "shared-secret"
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let config = DaemonConfig::load(&config_path).await.unwrap();
        assert_eq!(
            config
                .http_auth
                .as_ref()
                .map(|auth| auth.bearer_token.as_str()),
            Some("shared-secret")
        );
    }

    #[tokio::test]
    async fn load_rejects_empty_http_bearer_auth() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[http_auth]
bearer_token = ""
"#,
                neutral_workdir(&dir)
            ),
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("http_auth.bearer_token must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_rejects_pinned_client_cert_for_http_transport() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        let daemon_cert = dir.path().join("daemon.pem");
        let daemon_key = dir.path().join("daemon.key");
        let ca_cert = dir.path().join("ca.pem");
        let broker_cert = dir.path().join("broker.pem");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:8080"
default_workdir = {}
transport = "http"

[tls]
cert_pem = {}
key_pem = {}
ca_pem = {}
pinned_client_cert_pem = {}
"#,
                neutral_workdir(&dir),
                neutral_toml_path(&daemon_cert),
                neutral_toml_path(&daemon_key),
                neutral_toml_path(&ca_cert),
                neutral_toml_path(&broker_cert)
            ),
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

    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn load_rejects_explicit_tls_transport_when_tls_feature_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        let daemon_cert = dir.path().join("daemon.pem");
        let daemon_key = dir.path().join("daemon.key");
        let ca_cert = dir.path().join("ca.pem");
        tokio::fs::write(
            &config_path,
            format!(
                r#"
target = "builder-a"
listen = "127.0.0.1:9443"
default_workdir = {}
transport = "tls"

[tls]
cert_pem = {}
key_pem = {}
ca_pem = {}
"#,
                neutral_workdir(&dir),
                neutral_toml_path(&daemon_cert),
                neutral_toml_path(&daemon_key),
                neutral_toml_path(&ca_cert)
            ),
        )
        .await
        .unwrap();

        let err = DaemonConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains(crate::tls::FEATURE_REQUIRED_MESSAGE),
            "unexpected error: {err}"
        );
    }
}
