pub(crate) mod broker_tls;
pub mod client;
pub mod config;
pub mod daemon_client;
pub mod local_backend;
pub mod local_transfer;
pub mod logging;
pub mod mcp_server;
pub mod session_store;
pub mod tools;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context;
use daemon_client::{DaemonClient, DaemonClientError};
use local_backend::LocalDaemonClient;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferSourceType,
};
use remote_exec_proto::{
    path::{PathPolicy, linux_path_policy, windows_path_policy},
    sandbox::{CompiledFilesystemSandbox, compile_filesystem_sandbox},
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
    pub supports_transfer_compression: bool,
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
    fn new(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
        identity_verified: bool,
        cached_daemon_info: Option<CachedDaemonInfo>,
    ) -> Self {
        Self {
            backend,
            expected_daemon_name,
            identity_verified: Arc::new(Mutex::new(identity_verified)),
            cached_daemon_info: Arc::new(Mutex::new(cached_daemon_info)),
        }
    }

    fn verified(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
        info: &TargetInfoResponse,
    ) -> Self {
        Self::new(
            backend,
            expected_daemon_name,
            true,
            Some(Self::cache_from_target_info(info)),
        )
    }

    fn unavailable(backend: TargetBackend, expected_daemon_name: Option<String>) -> Self {
        Self::new(backend, expected_daemon_name, false, None)
    }

    fn cache_from_target_info(info: &TargetInfoResponse) -> CachedDaemonInfo {
        CachedDaemonInfo {
            daemon_version: info.daemon_version.clone(),
            hostname: info.hostname.clone(),
            platform: info.platform.clone(),
            arch: info.arch.clone(),
            supports_pty: info.supports_pty,
            supports_transfer_compression: info.supports_transfer_compression,
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
        ensure_expected_daemon_name(name, self.expected_daemon_name.as_deref(), &info.target)?;

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

fn ensure_expected_daemon_name(
    target_name: &str,
    expected_daemon_name: Option<&str>,
    actual_daemon_name: &str,
) -> anyhow::Result<()> {
    if let Some(expected_daemon_name) = expected_daemon_name {
        anyhow::ensure!(
            actual_daemon_name == expected_daemon_name,
            "target `{target_name}` resolved to daemon `{actual_daemon_name}` instead of `{expected_daemon_name}`"
        );
    }

    Ok(())
}

#[derive(Clone)]
pub struct BrokerState {
    pub enable_transfer_compression: bool,
    pub disable_structured_content: bool,
    pub host_sandbox: Option<CompiledFilesystemSandbox>,
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
    let mcp = config.mcp.clone();
    tracing::info!(
        configured_targets = config.targets.len(),
        local_target_enabled = config.local.is_some(),
        disable_structured_content = config.disable_structured_content,
        mcp_transport = mcp_transport_name(&mcp),
        "starting broker"
    );
    let state = build_state(config).await?;
    tracing::info!(configured_targets = state.targets.len(), "broker ready");
    mcp_server::serve(state, &mcp).await
}

async fn build_state(mut config: config::BrokerConfig) -> anyhow::Result<BrokerState> {
    config.normalize_paths();
    config.validate()?;
    let host_sandbox = compile_host_sandbox(&config)?;
    let mut targets = BTreeMap::new();

    insert_local_target(&config, &mut targets).await?;
    insert_remote_targets(&config.targets, &mut targets).await?;

    Ok(BrokerState {
        enable_transfer_compression: config.enable_transfer_compression,
        disable_structured_content: config.disable_structured_content,
        host_sandbox,
        sessions: SessionStore::default(),
        targets,
    })
}

fn compile_host_sandbox(
    config: &config::BrokerConfig,
) -> anyhow::Result<Option<CompiledFilesystemSandbox>> {
    Ok(config
        .host_sandbox
        .as_ref()
        .map(|sandbox| compile_filesystem_sandbox(host_path_policy(), sandbox))
        .transpose()?)
}

async fn insert_local_target(
    config: &config::BrokerConfig,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    let Some(local_config) = &config.local else {
        return Ok(());
    };

    let client = LocalDaemonClient::new(
        local_config,
        config.host_sandbox.clone(),
        config.enable_transfer_compression,
    )?;
    let info = client.target_info().await?;
    log_local_target_enabled(&info);
    targets.insert(
        "local".to_string(),
        TargetHandle::verified(
            TargetBackend::Local(client),
            Some("local".to_string()),
            &info,
        ),
    );
    Ok(())
}

async fn insert_remote_targets(
    target_configs: &BTreeMap<String, config::TargetConfig>,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    for (name, target_config) in target_configs {
        let handle = build_remote_target_handle(name, target_config).await?;
        targets.insert(name.clone(), handle);
    }
    Ok(())
}

async fn build_remote_target_handle(
    name: &str,
    target_config: &config::TargetConfig,
) -> anyhow::Result<TargetHandle> {
    let client = DaemonClient::new(name.to_string(), target_config).await?;
    match client.target_info().await {
        Ok(info) => {
            ensure_expected_daemon_name(
                name,
                target_config.expected_daemon_name.as_deref(),
                &info.target,
            )?;
            log_remote_target_available(name, target_config, &info);
            Ok(TargetHandle::verified(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
                &info,
            ))
        }
        Err(DaemonClientError::Transport(err)) => {
            log_remote_target_unavailable(name, target_config, &err);
            Ok(TargetHandle::unavailable(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Err(err) => Err(err.into()),
    }
}

fn log_local_target_enabled(info: &TargetInfoResponse) {
    tracing::info!(
        target = "local",
        daemon_instance_id = %info.daemon_instance_id,
        platform = %info.platform,
        arch = %info.arch,
        hostname = %info.hostname,
        supports_pty = info.supports_pty,
        supports_transfer_compression = info.supports_transfer_compression,
        "enabled embedded local target"
    );
}

fn log_remote_target_available(
    name: &str,
    target_config: &config::TargetConfig,
    info: &TargetInfoResponse,
) {
    tracing::info!(
        target = %name,
        base_url = %target_config.base_url,
        http_auth_enabled = target_config.http_auth.is_some(),
        daemon_name = %info.target,
        daemon_instance_id = %info.daemon_instance_id,
        platform = %info.platform,
        arch = %info.arch,
        hostname = %info.hostname,
        supports_pty = info.supports_pty,
        supports_transfer_compression = info.supports_transfer_compression,
        "target available during broker startup"
    );
}

fn log_remote_target_unavailable(
    name: &str,
    target_config: &config::TargetConfig,
    err: &anyhow::Error,
) {
    tracing::warn!(
        target = %name,
        http_auth_enabled = target_config.http_auth.is_some(),
        ?err,
        "target unavailable during broker startup"
    );
}

fn host_path_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn unsupported_local_transfer_error() -> DaemonClientError {
    DaemonClientError::Rpc {
        status: reqwest::StatusCode::BAD_REQUEST,
        code: Some("unsupported_operation".to_string()),
        message: "embedded local target does not use daemon transfer RPC".to_string(),
    }
}

fn mcp_transport_name(config: &config::McpServerConfig) -> &'static str {
    match config {
        config::McpServerConfig::Stdio => "stdio",
        config::McpServerConfig::StreamableHttp { .. } => "streamable_http",
    }
}

pub fn install_crypto_provider() {
    remote_exec_daemon::install_crypto_provider();
    broker_tls::install_crypto_provider();
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
            mcp: Default::default(),
            host_sandbox: None,
            enable_transfer_compression: true,
            disable_structured_content: false,
            targets: BTreeMap::new(),
            local: Some(LocalTargetConfig {
                default_workdir: tempdir.path().to_path_buf(),
                windows_posix_root: None,
                allow_login_shell: true,
                pty: remote_exec_daemon::config::PtyMode::Auto,
                default_shell: Some(missing_shell.to_string()),
                yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
                experimental_apply_patch_target_encoding_autodetect: false,
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
