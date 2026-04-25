use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use remote_exec_proto::sandbox::FilesystemSandbox;
use serde::Deserialize;

mod environment;
mod yield_time;

#[cfg(test)]
mod tests;

pub use environment::ProcessEnvironment;
pub use yield_time::{YieldTimeConfig, YieldTimeOperation, YieldTimeOperationConfig};

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
