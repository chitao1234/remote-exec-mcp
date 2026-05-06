pub mod config;
pub mod exec;
pub mod http;
pub mod image;
pub mod logging;
pub mod patch;
pub mod port_forward;
pub(crate) mod rpc_error;
pub mod server;
pub mod tls;
pub mod transfer;

use std::future::Future;
use std::future::pending;
use std::sync::Arc;

use anyhow::Result;
use config::DaemonConfig;
use remote_exec_proto::rpc::TargetInfoResponse;

pub type AppState = remote_exec_host::HostRuntimeState;

pub async fn run(config: DaemonConfig) -> Result<()> {
    run_until(config, pending::<()>()).await
}

pub fn install_crypto_provider() {
    tls::install_crypto_provider();
}

pub fn build_app_state(config: DaemonConfig) -> Result<AppState> {
    remote_exec_host::build_runtime_state(config.into_host_runtime_config())
}

pub fn target_info_response(state: &AppState) -> TargetInfoResponse {
    remote_exec_host::target_info_response(state, env!("CARGO_PKG_VERSION"))
}

pub async fn run_until<F>(config: DaemonConfig, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    tls::install_crypto_provider();
    let daemon_config = Arc::new(config);
    let state = remote_exec_host::build_runtime_state(daemon_config.host_runtime_config())?;
    tracing::info!(
        target = %daemon_config.target,
        listen = %daemon_config.listen,
        transport = ?daemon_config.transport,
        http_auth_enabled = daemon_config.http_auth.is_some(),
        default_workdir = %daemon_config.default_workdir.display(),
        default_shell = %state.default_shell,
        supports_pty = state.supports_pty,
        supports_transfer_compression = state.supports_transfer_compression,
        pty_mode = ?daemon_config.pty,
        daemon_instance_id = %state.daemon_instance_id,
        "starting daemon"
    );
    let shutdown_state = state.clone();
    let shutdown = async move {
        shutdown.await;
        shutdown_state.shutdown.cancel();
    };
    server::serve_with_shutdown(state, daemon_config, shutdown).await
}
