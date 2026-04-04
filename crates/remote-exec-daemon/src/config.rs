use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPtyBackendOverride {
    PortablePty,
    Winpty,
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
    #[serde(default = "default_allow_login_shell")]
    pub allow_login_shell: bool,
    #[serde(skip, default)]
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    #[serde(skip, default = "ProcessEnvironment::capture_current")]
    pub process_environment: ProcessEnvironment,
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
