pub mod config;
pub mod exec;
pub mod image;
pub mod logging;
pub mod patch;
pub mod server;
pub mod tls;
pub mod transfer;

use std::future::Future;
use std::future::pending;
use std::sync::Arc;
use std::sync::Once;

use anyhow::Result;
use config::{DaemonConfig, WindowsPtyBackendOverride};
use remote_exec_proto::{
    path::{PathPolicy, linux_path_policy, windows_path_policy},
    rpc::TargetInfoResponse,
    sandbox::{CompiledFilesystemSandbox, compile_filesystem_sandbox},
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<DaemonConfig>,
    pub default_shell: String,
    pub sandbox: Option<CompiledFilesystemSandbox>,
    pub supports_pty: bool,
    pub supports_transfer_compression: bool,
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    pub daemon_instance_id: String,
    pub sessions: exec::store::SessionStore,
}

pub async fn run(config: DaemonConfig) -> Result<()> {
    run_until(config, pending::<()>()).await
}

pub fn build_app_state(config: DaemonConfig) -> Result<AppState> {
    config.validate()?;
    let sandbox = config
        .sandbox
        .as_ref()
        .map(|sandbox| compile_filesystem_sandbox(host_path_policy(), sandbox))
        .transpose()?;
    let default_shell = exec::shell::resolve_default_shell(
        config.default_shell.as_deref(),
        &config.process_environment,
    )?;
    exec::session::validate_pty_mode(config.pty)?;
    let supports_pty = exec::session::supports_pty_for_mode(config.pty);
    let supports_transfer_compression = config.enable_transfer_compression;
    let windows_pty_backend_override =
        exec::session::windows_pty_backend_override_for_mode(config.pty)?;

    Ok(AppState {
        config: Arc::new(config),
        default_shell,
        sandbox,
        supports_pty,
        supports_transfer_compression,
        windows_pty_backend_override,
        daemon_instance_id: uuid::Uuid::new_v4().to_string(),
        sessions: exec::store::SessionStore::new(64),
    })
}

fn host_path_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

pub fn target_info_response(state: &AppState) -> TargetInfoResponse {
    TargetInfoResponse {
        target: state.config.target.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        supports_pty: state.supports_pty,
        supports_image_read: true,
        supports_transfer_compression: state.supports_transfer_compression,
    }
}

pub async fn run_until<F>(config: DaemonConfig, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    install_crypto_provider();
    let state = build_app_state(config)?;
    tracing::info!(
        target = %state.config.target,
        listen = %state.config.listen,
        transport = ?state.config.transport,
        default_workdir = %state.config.default_workdir.display(),
        default_shell = %state.default_shell,
        supports_pty = state.supports_pty,
        supports_transfer_compression = state.supports_transfer_compression,
        pty_mode = ?state.config.pty,
        daemon_instance_id = %state.daemon_instance_id,
        "starting daemon"
    );
    server::serve_with_shutdown(state, shutdown).await
}

pub fn install_crypto_provider() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let provider = rustls::crypto::ring::default_provider();
        provider
            .install_default()
            .expect("failed to install rustls crypto provider");
    });
}
