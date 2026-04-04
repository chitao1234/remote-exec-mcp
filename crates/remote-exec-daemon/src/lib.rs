pub mod config;
pub mod exec;
pub mod image;
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

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<DaemonConfig>,
    pub default_shell: String,
    pub supports_pty: bool,
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    pub daemon_instance_id: String,
    pub sessions: exec::store::SessionStore,
}

pub async fn run(config: DaemonConfig) -> Result<()> {
    run_until(config, pending::<()>()).await
}

pub async fn run_until<F>(config: DaemonConfig, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    install_crypto_provider();

    let default_shell = exec::shell::resolve_default_shell(
        config.default_shell.as_deref(),
        &config.process_environment,
    )?;
    exec::session::validate_pty_mode(config.pty)?;
    let supports_pty = exec::session::supports_pty_for_mode(config.pty);
    let windows_pty_backend_override =
        exec::session::windows_pty_backend_override_for_mode(config.pty)?;

    let state = AppState {
        config: Arc::new(config),
        default_shell,
        supports_pty,
        windows_pty_backend_override,
        daemon_instance_id: uuid::Uuid::new_v4().to_string(),
        sessions: exec::store::SessionStore::new(64),
    };
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
