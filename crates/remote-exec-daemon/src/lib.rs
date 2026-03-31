pub mod config;
pub mod exec;
pub mod image;
pub mod patch;
pub mod server;
pub mod tls;

use std::sync::Arc;
use std::sync::Once;

use anyhow::Result;
use config::DaemonConfig;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<DaemonConfig>,
    pub daemon_instance_id: String,
    pub sessions: exec::store::SessionStore,
}

pub async fn run(config: DaemonConfig) -> Result<()> {
    install_crypto_provider();

    let state = AppState {
        config: Arc::new(config),
        daemon_instance_id: uuid::Uuid::new_v4().to_string(),
        sessions: exec::store::SessionStore::default(),
    };
    server::serve(state).await
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
