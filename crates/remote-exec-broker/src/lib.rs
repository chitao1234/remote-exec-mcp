pub mod config;
pub mod daemon_client;
pub mod local_transfer;
pub mod mcp_server;
pub mod session_store;
pub mod tools;

use std::collections::BTreeMap;
use std::sync::{Arc, Once};

use anyhow::Context;
use daemon_client::{DaemonClient, DaemonClientError};
use remote_exec_proto::rpc::TargetInfoResponse;
use session_store::SessionStore;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
}

#[derive(Clone)]
pub struct TargetHandle {
    pub client: DaemonClient,
    expected_daemon_name: Option<String>,
    identity_verified: Arc<Mutex<bool>>,
    cached_daemon_info: Arc<Mutex<Option<CachedDaemonInfo>>>,
}

impl TargetHandle {
    fn cache_from_target_info(info: &TargetInfoResponse) -> CachedDaemonInfo {
        CachedDaemonInfo {
            daemon_version: info.daemon_version.clone(),
            hostname: info.hostname.clone(),
            platform: info.platform.clone(),
            arch: info.arch.clone(),
            supports_pty: info.supports_pty,
        }
    }

    pub async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.cached_daemon_info.lock().await.clone()
    }

    pub async fn clear_cached_daemon_info(&self) {
        *self.identity_verified.lock().await = false;
        *self.cached_daemon_info.lock().await = None;
    }

    pub async fn ensure_identity_verified(&self, name: &str) -> anyhow::Result<()> {
        let mut identity_verified = self.identity_verified.lock().await;
        if *identity_verified {
            return Ok(());
        }

        let info = match self.client.target_info().await {
            Ok(info) => info,
            Err(DaemonClientError::Transport(err)) => {
                *identity_verified = false;
                *self.cached_daemon_info.lock().await = None;
                return Err(DaemonClientError::Transport(err).into());
            }
            Err(err) => return Err(err.into()),
        };
        if let Some(expected_name) = &self.expected_daemon_name {
            anyhow::ensure!(
                &info.target == expected_name,
                "target `{name}` resolved to daemon `{}` instead of `{expected_name}`",
                info.target
            );
        }

        *self.cached_daemon_info.lock().await = Some(Self::cache_from_target_info(&info));
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
    config.validate()?;
    let mut targets = BTreeMap::new();

    for (name, target_config) in &config.targets {
        let client = DaemonClient::new(target_config).await?;
        let (identity_verified, cached_daemon_info) = match client.target_info().await {
            Ok(info) => {
                if let Some(expected_name) = &target_config.expected_daemon_name {
                    anyhow::ensure!(
                        &info.target == expected_name,
                        "target `{name}` resolved to daemon `{}` instead of `{expected_name}`",
                        info.target
                    );
                }
                (true, Some(TargetHandle::cache_from_target_info(&info)))
            }
            Err(DaemonClientError::Transport(err)) => {
                tracing::warn!(target = %name, ?err, "target unavailable during broker startup");
                (false, None)
            }
            Err(err) => return Err(err.into()),
        };

        targets.insert(
            name.clone(),
            TargetHandle {
                client,
                expected_daemon_name: target_config.expected_daemon_name.clone(),
                identity_verified: Arc::new(Mutex::new(identity_verified)),
                cached_daemon_info: Arc::new(Mutex::new(cached_daemon_info)),
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
