use std::path::{Path, PathBuf};

use anyhow::Context;
use remote_exec_proto::sandbox::FilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;
use serde::Deserialize;

mod environment;
mod yield_time;

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

#[derive(Debug, Clone)]
pub struct HostRuntimeConfig {
    pub target: String,
    pub default_workdir: PathBuf,
    pub windows_posix_root: Option<PathBuf>,
    pub sandbox: Option<FilesystemSandbox>,
    pub enable_transfer_compression: bool,
    pub transfer_limits: TransferLimits,
    pub allow_login_shell: bool,
    pub pty: PtyMode,
    pub default_shell: Option<String>,
    pub yield_time: YieldTimeConfig,
    pub port_forward_limits: HostPortForwardLimits,
    pub experimental_apply_patch_target_encoding_autodetect: bool,
    pub process_environment: ProcessEnvironment,
}

#[derive(Debug, Clone)]
pub struct EmbeddedHostConfig {
    pub target: String,
    pub default_workdir: PathBuf,
    pub windows_posix_root: Option<PathBuf>,
    pub sandbox: Option<FilesystemSandbox>,
    pub enable_transfer_compression: bool,
    pub transfer_limits: TransferLimits,
    pub allow_login_shell: bool,
    pub pty: PtyMode,
    pub default_shell: Option<String>,
    pub yield_time: YieldTimeConfig,
    pub port_forward_limits: HostPortForwardLimits,
    pub experimental_apply_patch_target_encoding_autodetect: bool,
    pub process_environment: ProcessEnvironment,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct HostPortForwardLimits {
    pub max_tunnel_connections: usize,
    pub max_retained_sessions: usize,
    pub max_retained_listeners: usize,
    pub max_udp_binds: usize,
    pub max_active_tcp_streams: usize,
    pub max_tunnel_queued_bytes: usize,
    pub connect_timeout_ms: u64,
}

impl Default for HostPortForwardLimits {
    fn default() -> Self {
        Self {
            max_tunnel_connections: 128,
            max_retained_sessions: 64,
            max_retained_listeners: 64,
            max_udp_binds: 64,
            max_active_tcp_streams: 1024,
            max_tunnel_queued_bytes: 8 * 1024 * 1024,
            connect_timeout_ms: 10_000,
        }
    }
}

impl HostPortForwardLimits {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_tunnel_connections > 0,
            "port_forward_limits.max_tunnel_connections must be greater than zero"
        );
        anyhow::ensure!(
            self.max_retained_sessions > 0,
            "port_forward_limits.max_retained_sessions must be greater than zero"
        );
        anyhow::ensure!(
            self.max_retained_listeners > 0,
            "port_forward_limits.max_retained_listeners must be greater than zero"
        );
        anyhow::ensure!(
            self.max_udp_binds > 0,
            "port_forward_limits.max_udp_binds must be greater than zero"
        );
        anyhow::ensure!(
            self.max_active_tcp_streams > 0,
            "port_forward_limits.max_active_tcp_streams must be greater than zero"
        );
        anyhow::ensure!(
            self.max_tunnel_queued_bytes > 0,
            "port_forward_limits.max_tunnel_queued_bytes must be greater than zero"
        );
        anyhow::ensure!(
            self.connect_timeout_ms > 0,
            "port_forward_limits.connect_timeout_ms must be greater than zero"
        );
        Ok(())
    }
}

impl EmbeddedHostConfig {
    pub fn into_host_runtime_config(self) -> HostRuntimeConfig {
        HostRuntimeConfig {
            target: self.target,
            default_workdir: self.default_workdir,
            windows_posix_root: self.windows_posix_root,
            sandbox: self.sandbox,
            enable_transfer_compression: self.enable_transfer_compression,
            transfer_limits: self.transfer_limits,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell,
            yield_time: self.yield_time,
            port_forward_limits: self.port_forward_limits,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: self.process_environment,
        }
    }
}

impl HostRuntimeConfig {
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

    pub fn normalize_paths(&mut self) {
        self.default_workdir = self.normalized_default_workdir();
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_windows_posix_root()?;
        validate_existing_directory(&self.normalized_default_workdir(), "default_workdir")?;
        self.yield_time.validate()?;
        self.transfer_limits.validate()?;
        self.port_forward_limits.validate()?;
        Ok(())
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
