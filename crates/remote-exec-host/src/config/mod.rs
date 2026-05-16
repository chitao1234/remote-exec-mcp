use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES;
use remote_exec_proto::sandbox::FilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;
use serde::Deserialize;

mod environment;
mod yield_time;

pub use environment::ProcessEnvironment;
pub use yield_time::{YieldTimeConfig, YieldTimeOperation, YieldTimeOperationConfig};

pub const DEFAULT_MAX_OPEN_SESSIONS: usize = 64;

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
    pub max_open_sessions: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPortForwardCapacityLimits {
    pub max_tunnel_connections: usize,
    pub max_retained_sessions: usize,
    pub max_retained_listeners: usize,
    pub max_udp_binds: usize,
    pub max_active_tcp_streams: usize,
    pub max_tunnel_queued_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPortForwardTimeouts {
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
            max_tunnel_queued_bytes: DEFAULT_TUNNEL_QUEUE_BYTES as usize,
            connect_timeout_ms: 10_000,
        }
    }
}

impl HostPortForwardLimits {
    pub fn capacity(self) -> HostPortForwardCapacityLimits {
        HostPortForwardCapacityLimits {
            max_tunnel_connections: self.max_tunnel_connections,
            max_retained_sessions: self.max_retained_sessions,
            max_retained_listeners: self.max_retained_listeners,
            max_udp_binds: self.max_udp_binds,
            max_active_tcp_streams: self.max_active_tcp_streams,
            max_tunnel_queued_bytes: self.max_tunnel_queued_bytes,
        }
    }

    pub fn timeouts(self) -> HostPortForwardTimeouts {
        HostPortForwardTimeouts {
            connect_timeout_ms: self.connect_timeout_ms,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.capacity().validate()?;
        self.timeouts().validate()?;
        Ok(())
    }
}

impl HostPortForwardCapacityLimits {
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
        Ok(())
    }
}

impl HostPortForwardTimeouts {
    pub fn connect_timeout(self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.connect_timeout_ms > 0,
            "port_forward_limits.connect_timeout_ms must be greater than zero"
        );
        Ok(())
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
        anyhow::ensure!(
            self.max_open_sessions > 0,
            "max_open_sessions must be greater than zero"
        );
        self.port_forward_limits.validate()?;
        Ok(())
    }
}

pub fn normalize_configured_workdir(path: &Path, windows_posix_root: Option<&Path>) -> PathBuf {
    crate::host_path::resolve_absolute_input_path(&path.to_string_lossy(), windows_posix_root)
        .unwrap_or_else(|| path.to_path_buf())
}

pub fn validate_existing_directory(path: &Path, field_name: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("{field_name} `{}` does not exist", path.display()))?;
    anyhow::ensure!(
        metadata.is_dir(),
        "{field_name} `{}` must be a directory",
        path.display()
    );
    Ok(())
}
