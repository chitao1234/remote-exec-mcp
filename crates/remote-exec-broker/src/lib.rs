pub mod config;
pub mod daemon_client;
pub mod mcp_server;
pub mod session_store;
pub mod tools;

use std::collections::BTreeMap;
use std::sync::{Arc, Once};

use anyhow::Context;
use daemon_client::{DaemonClient, DaemonClientError};
use session_store::SessionStore;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct TargetHandle {
    pub client: DaemonClient,
    expected_daemon_name: Option<String>,
    identity_verified: Arc<Mutex<bool>>,
}

impl TargetHandle {
    pub async fn ensure_identity_verified(&self, name: &str) -> anyhow::Result<()> {
        let mut identity_verified = self.identity_verified.lock().await;
        if *identity_verified {
            return Ok(());
        }

        let info = self.client.target_info().await?;
        if let Some(expected_name) = &self.expected_daemon_name {
            anyhow::ensure!(
                &info.target == expected_name,
                "target `{name}` resolved to daemon `{}` instead of `{expected_name}`",
                info.target
            );
        }

        *identity_verified = true;
        Ok(())
    }
}

#[derive(Clone)]
pub struct BrokerState {
    pub sessions: SessionStore,
    pub targets: BTreeMap<String, TargetHandle>,
}

impl BrokerState {
    pub fn target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .with_context(|| format!("unknown target `{name}`"))
    }
}

pub async fn run(config: config::BrokerConfig) -> anyhow::Result<()> {
    install_crypto_provider();
    let state = build_state(config).await?;
    mcp_server::serve_stdio(state).await
}

async fn build_state(config: config::BrokerConfig) -> anyhow::Result<BrokerState> {
    let mut targets = BTreeMap::new();

    for (name, target_config) in &config.targets {
        let client = DaemonClient::new(target_config).await?;
        let identity_verified = match client.target_info().await {
            Ok(info) => {
                if let Some(expected_name) = &target_config.expected_daemon_name {
                    anyhow::ensure!(
                        &info.target == expected_name,
                        "target `{name}` resolved to daemon `{}` instead of `{expected_name}`",
                        info.target
                    );
                }
                true
            }
            Err(DaemonClientError::Transport(err)) => {
                tracing::warn!(target = %name, ?err, "target unavailable during broker startup");
                false
            }
            Err(err) => return Err(err.into()),
        };

        targets.insert(
            name.clone(),
            TargetHandle {
                client,
                expected_daemon_name: target_config.expected_daemon_name.clone(),
                identity_verified: Arc::new(Mutex::new(identity_verified)),
            },
        );
    }

    Ok(BrokerState {
        sessions: SessionStore::default(),
        targets,
    })
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
