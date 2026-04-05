pub mod config;
pub mod daemon_client;
pub mod local_backend;
pub mod local_transfer;
pub mod logging;
pub mod mcp_server;
pub mod session_store;
pub mod tools;

use std::collections::BTreeMap;
use std::sync::{Arc, Once};

use anyhow::Context;
use daemon_client::{DaemonClient, DaemonClientError};
use local_backend::LocalDaemonClient;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferSourceType,
};
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
enum TargetBackend {
    Remote(DaemonClient),
    Local(LocalDaemonClient),
}

#[derive(Clone)]
pub struct TargetHandle {
    backend: TargetBackend,
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

    pub async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.target_info().await,
            TargetBackend::Local(client) => client.target_info().await,
        }
    }

    pub async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.exec_start(req).await,
            TargetBackend::Local(client) => client.exec_start(req).await,
        }
    }

    pub async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.exec_write(req).await,
            TargetBackend::Local(client) => client.exec_write(req).await,
        }
    }

    pub async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.patch_apply(req).await,
            TargetBackend::Local(client) => client.patch_apply(req).await,
        }
    }

    pub async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.image_read(req).await,
            TargetBackend::Local(client) => client.image_read(req).await,
        }
    }

    pub async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferSourceType, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => {
                client.transfer_export_to_file(req, archive_path).await
            }
            TargetBackend::Local(_) => Err(unsupported_local_transfer_error()),
        }
    }

    pub async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => {
                client.transfer_import_from_file(archive_path, req).await
            }
            TargetBackend::Local(_) => Err(unsupported_local_transfer_error()),
        }
    }

    pub async fn clear_cached_daemon_info(&self) {
        *self.identity_verified.lock().await = false;
        *self.cached_daemon_info.lock().await = None;
        tracing::info!("cleared cached daemon identity and metadata");
    }

    pub async fn ensure_identity_verified(&self, name: &str) -> anyhow::Result<()> {
        let mut identity_verified = self.identity_verified.lock().await;
        if *identity_verified {
            return Ok(());
        }

        let info = match self.target_info().await {
            Ok(info) => info,
            Err(DaemonClientError::Transport(err)) => {
                *identity_verified = false;
                *self.cached_daemon_info.lock().await = None;
                tracing::warn!(target = %name, ?err, "target identity verification failed");
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
        tracing::info!(
            target = %name,
            daemon_name = %info.target,
            daemon_instance_id = %info.daemon_instance_id,
            platform = %info.platform,
            arch = %info.arch,
            hostname = %info.hostname,
            supports_pty = info.supports_pty,
            "verified target identity"
        );
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
    tracing::info!(
        configured_targets = config.targets.len(),
        local_target_enabled = config.local.is_some(),
        "starting broker"
    );
    let state = build_state(config).await?;
    tracing::info!(configured_targets = state.targets.len(), "broker ready");
    mcp_server::serve_stdio(state).await
}

async fn build_state(config: config::BrokerConfig) -> anyhow::Result<BrokerState> {
    config.validate()?;
    let mut targets = BTreeMap::new();

    if let Some(local_config) = &config.local {
        let client = LocalDaemonClient::new(local_config)?;
        let info = client.target_info().await?;
        tracing::info!(
            target = "local",
            daemon_instance_id = %info.daemon_instance_id,
            platform = %info.platform,
            arch = %info.arch,
            hostname = %info.hostname,
            supports_pty = info.supports_pty,
            "enabled embedded local target"
        );
        targets.insert(
            "local".to_string(),
            TargetHandle {
                backend: TargetBackend::Local(client),
                expected_daemon_name: Some("local".to_string()),
                identity_verified: Arc::new(Mutex::new(true)),
                cached_daemon_info: Arc::new(Mutex::new(Some(
                    TargetHandle::cache_from_target_info(&info),
                ))),
            },
        );
    }

    for (name, target_config) in &config.targets {
        let client = DaemonClient::new(name.clone(), target_config).await?;
        let (identity_verified, cached_daemon_info) = match client.target_info().await {
            Ok(info) => {
                if let Some(expected_name) = &target_config.expected_daemon_name {
                    anyhow::ensure!(
                        &info.target == expected_name,
                        "target `{name}` resolved to daemon `{}` instead of `{expected_name}`",
                        info.target
                    );
                }
                tracing::info!(
                    target = %name,
                    base_url = %target_config.base_url,
                    daemon_name = %info.target,
                    daemon_instance_id = %info.daemon_instance_id,
                    platform = %info.platform,
                    arch = %info.arch,
                    hostname = %info.hostname,
                    supports_pty = info.supports_pty,
                    "target available during broker startup"
                );
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
                backend: TargetBackend::Remote(client),
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

fn unsupported_local_transfer_error() -> DaemonClientError {
    DaemonClientError::Rpc {
        status: reqwest::StatusCode::BAD_REQUEST,
        code: Some("unsupported_operation".to_string()),
        message: "embedded local target does not use daemon transfer RPC".to_string(),
    }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::build_state;
    use crate::config::{BrokerConfig, LocalTargetConfig};

    #[tokio::test]
    async fn build_state_rejects_unusable_local_default_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let missing_shell = "/definitely/missing/remote-exec-shell";
        #[cfg(windows)]
        let missing_shell = r"C:\definitely\missing\remote-exec-shell.exe";

        let err = match build_state(BrokerConfig {
            targets: BTreeMap::new(),
            local: Some(LocalTargetConfig {
                default_workdir: tempdir.path().to_path_buf(),
                allow_login_shell: true,
                pty: remote_exec_daemon::config::PtyMode::Auto,
                default_shell: Some(missing_shell.to_string()),
            }),
        })
        .await
        {
            Ok(_) => panic!("expected local default shell validation to fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("not found") || err.to_string().contains("usable"),
            "unexpected error: {err}"
        );
    }
}
