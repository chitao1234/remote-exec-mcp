use std::sync::Arc;

use remote_exec_proto::{
    rpc::TargetInfoResponse,
    sandbox::{CompiledFilesystemSandbox, compile_filesystem_sandbox},
};
use tokio_util::sync::CancellationToken;

use crate::{HostRuntimeConfig, WindowsPtyBackendOverride};

#[derive(Clone)]
pub struct HostRuntimeState {
    pub config: Arc<HostRuntimeConfig>,
    pub default_shell: String,
    pub sandbox: Option<CompiledFilesystemSandbox>,
    pub supports_pty: bool,
    pub supports_transfer_compression: bool,
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    pub daemon_instance_id: String,
    pub shutdown: CancellationToken,
    pub sessions: crate::exec::store::SessionStore,
    pub port_forward_sessions: crate::port_forward::TunnelSessionStore,
    pub port_forward_limiter: Arc<crate::port_forward::PortForwardLimiter>,
}

pub fn build_runtime_state(mut config: HostRuntimeConfig) -> anyhow::Result<HostRuntimeState> {
    config.normalize_paths();
    config.validate()?;
    let sandbox = config
        .sandbox
        .as_ref()
        .map(|sandbox| compile_filesystem_sandbox(crate::host_path::host_path_policy(), sandbox))
        .transpose()?;
    let default_shell = crate::exec::shell::resolve_default_shell(
        config.default_shell.as_deref(),
        &config.process_environment,
        config.windows_posix_root.as_deref(),
    )?;
    crate::exec::session::validate_pty_mode(config.pty)?;
    let supports_pty = crate::exec::session::supports_pty_for_mode(config.pty);
    let supports_transfer_compression = config.enable_transfer_compression;
    let windows_pty_backend_override =
        crate::exec::session::windows_pty_backend_override_for_mode(config.pty)?;

    let port_forward_limits = config.port_forward_limits;

    Ok(HostRuntimeState {
        config: Arc::new(config),
        default_shell,
        sandbox,
        supports_pty,
        supports_transfer_compression,
        windows_pty_backend_override,
        daemon_instance_id: uuid::Uuid::new_v4().to_string(),
        shutdown: CancellationToken::new(),
        sessions: crate::exec::store::SessionStore::new(64),
        port_forward_sessions: crate::port_forward::TunnelSessionStore::default(),
        port_forward_limiter: Arc::new(crate::port_forward::PortForwardLimiter::new(
            port_forward_limits,
        )),
    })
}

pub fn target_info_response(state: &HostRuntimeState, daemon_version: &str) -> TargetInfoResponse {
    TargetInfoResponse {
        target: state.config.target.clone(),
        daemon_version: daemon_version.to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        supports_pty: state.supports_pty,
        supports_image_read: true,
        supports_transfer_compression: state.supports_transfer_compression,
        supports_port_forward: true,
        port_forward_protocol_version: 4,
    }
}
