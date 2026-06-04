use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Context;
use remote_exec_proto::path::{PathPolicy, linux_path_policy, windows_path_policy};
use remote_exec_proto::rpc::{
    DaemonIdentity, ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest,
    FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse,
    HealthCheckResponse, ImageReadRequest, ImageReadResponse, PatchApplyRequest,
    PatchApplyResponse, TargetCapabilities, TargetInfoResponse, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferPathInfoRequest, TransferPathInfoResponse,
};
use tokio::sync::Mutex;

use crate::daemon_client::{DaemonClientError, TransferExportResponse, TransferExportStream};

use super::{TargetBackend, ensure_expected_daemon_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub target: String,
    pub daemon_instance_id: String,
    pub identity: DaemonIdentity,
    pub capabilities: TargetCapabilities,
    pub supports_transfer_compression: bool,
}

impl CachedDaemonInfo {
    pub(crate) fn platform_is_windows(&self) -> bool {
        self.identity.platform.eq_ignore_ascii_case("windows")
    }

    pub(crate) fn path_policy(&self) -> PathPolicy {
        if self.platform_is_windows() {
            windows_path_policy()
        } else {
            linux_path_policy()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTargetHealth {
    pub healthy: bool,
    pub daemon_version: Option<String>,
    pub daemon_instance_id: Option<String>,
    pub last_checked_at: SystemTime,
    pub last_error: Option<String>,
}

impl CachedTargetHealth {
    fn healthy(response: &HealthCheckResponse) -> Self {
        Self {
            healthy: true,
            daemon_version: Some(response.daemon_version.clone()),
            daemon_instance_id: Some(response.daemon_instance_id.clone()),
            last_checked_at: SystemTime::now(),
            last_error: None,
        }
    }

    fn unhealthy(error: &DaemonClientError) -> Self {
        Self {
            healthy: false,
            daemon_version: None,
            daemon_instance_id: None,
            last_checked_at: SystemTime::now(),
            last_error: Some(error.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct TargetHandle {
    pub(super) backend: TargetBackend,
    expected_daemon_name: Option<String>,
    cached_daemon_info: Arc<Mutex<Option<CachedDaemonInfo>>>,
    cached_health: Arc<Mutex<Option<CachedTargetHealth>>>,
}

#[derive(Clone, Copy)]
pub(crate) struct RemoteTargetHandle<'a> {
    handle: &'a TargetHandle,
    client: &'a crate::daemon_client::DaemonClient,
}

impl TargetHandle {
    fn new(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
        cached_daemon_info: Option<CachedDaemonInfo>,
        cached_health: Option<CachedTargetHealth>,
    ) -> Self {
        Self {
            backend,
            expected_daemon_name,
            cached_daemon_info: Arc::new(Mutex::new(cached_daemon_info)),
            cached_health: Arc::new(Mutex::new(cached_health)),
        }
    }

    pub(crate) fn verified(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
        info: &TargetInfoResponse,
    ) -> Self {
        Self::new(
            backend,
            expected_daemon_name,
            Some(Self::cache_from_target_info(info)),
            Some(CachedTargetHealth {
                healthy: true,
                daemon_version: Some(info.identity.daemon_version.clone()),
                daemon_instance_id: Some(info.daemon_instance_id.clone()),
                last_checked_at: SystemTime::now(),
                last_error: None,
            }),
        )
    }

    pub(crate) fn unavailable(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
    ) -> Self {
        Self::new(backend, expected_daemon_name, None, None)
    }

    fn cache_from_target_info(info: &TargetInfoResponse) -> CachedDaemonInfo {
        CachedDaemonInfo {
            target: info.target.clone(),
            daemon_instance_id: info.daemon_instance_id.clone(),
            identity: info.identity.clone(),
            capabilities: info.capabilities.clone(),
            supports_transfer_compression: info.supports_transfer_compression,
        }
    }

    pub(crate) async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.cached_daemon_info.lock().await.clone()
    }

    pub(crate) async fn verified_cached_daemon_info(
        &self,
        name: &str,
    ) -> anyhow::Result<CachedDaemonInfo> {
        self.ensure_daemon_info_cached(name).await?;
        self.cached_daemon_info()
            .await
            .context("target info missing after identity verification")
    }

    pub(crate) async fn health(&self) -> Result<HealthCheckResponse, DaemonClientError> {
        self.backend.health().await
    }

    pub(crate) async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        self.backend.target_info().await
    }

    pub(crate) async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.backend.exec_start(req).await
    }

    pub(crate) async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.backend.exec_write(req).await
    }

    pub(crate) async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        self.backend.patch_apply(req).await
    }

    pub(crate) async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        self.backend.image_read(req).await
    }

    pub(crate) async fn file_read(
        &self,
        req: &FileReadRequest,
    ) -> Result<FileReadResponse, DaemonClientError> {
        self.backend.file_read(req).await
    }

    pub(crate) async fn file_write(
        &self,
        req: &FileWriteRequest,
    ) -> Result<FileWriteResponse, DaemonClientError> {
        self.backend.file_write(req).await
    }

    pub(crate) async fn file_edit(
        &self,
        req: &FileEditRequest,
    ) -> Result<FileEditResponse, DaemonClientError> {
        self.backend.file_edit(req).await
    }

    pub(crate) fn as_remote(&self) -> Option<RemoteTargetHandle<'_>> {
        self.backend
            .remote_client()
            .map(|client| RemoteTargetHandle {
                handle: self,
                client,
            })
    }

    pub(crate) async fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> Result<crate::port_forward::PortTunnel, DaemonClientError> {
        self.backend.port_tunnel(max_queued_bytes).await
    }

    pub(crate) async fn invalidate_cached_daemon_info(&self) {
        *self.cached_daemon_info.lock().await = None;
        tracing::info!("cleared cached daemon identity and metadata");
    }

    pub(crate) async fn ensure_daemon_info_cached(&self, name: &str) -> anyhow::Result<()> {
        if self.cached_daemon_info.lock().await.is_some() {
            return Ok(());
        }

        let info = match self.target_info().await {
            Ok(info) => info,
            Err(DaemonClientError::Transport(err)) => {
                *self.cached_health.lock().await =
                    Some(CachedTargetHealth::unhealthy(&DaemonClientError::Transport(
                        anyhow::anyhow!(err.to_string()),
                    )));
                tracing::warn!(target = %name, ?err, "target identity verification failed");
                return Err(DaemonClientError::Transport(err).into());
            }
            Err(err) => return Err(err.into()),
        };
        ensure_expected_daemon_name(name, self.expected_daemon_name.as_deref(), &info.target)?;

        *self.cached_daemon_info.lock().await = Some(Self::cache_from_target_info(&info));
        *self.cached_health.lock().await = Some(CachedTargetHealth {
            healthy: true,
            daemon_version: Some(info.identity.daemon_version.clone()),
            daemon_instance_id: Some(info.daemon_instance_id.clone()),
            last_checked_at: SystemTime::now(),
            last_error: None,
        });
        tracing::info!(
            target = %name,
            daemon_name = %info.target,
            daemon_instance_id = %info.daemon_instance_id,
            platform = %info.identity.platform,
            arch = %info.identity.arch,
            hostname = %info.identity.hostname,
            supports_pty = info.capabilities.supports_pty,
            "verified target identity"
        );
        Ok(())
    }

    pub(crate) async fn refresh_health_and_cache(&self, name: &str) -> anyhow::Result<bool> {
        match self.health().await {
            Ok(health) => {
                *self.cached_health.lock().await = Some(CachedTargetHealth::healthy(&health));

                let existing = self.cached_daemon_info().await;
                let instance_changed = existing
                    .as_ref()
                    .map(|info| info.daemon_instance_id != health.daemon_instance_id)
                    .unwrap_or(false);

                if existing.is_none() || instance_changed {
                    if instance_changed {
                        self.invalidate_cached_daemon_info().await;
                    }
                    let info = self.target_info().await?;
                    ensure_expected_daemon_name(
                        name,
                        self.expected_daemon_name.as_deref(),
                        &info.target,
                    )?;
                    *self.cached_daemon_info.lock().await = Some(Self::cache_from_target_info(&info));
                }

                Ok(instance_changed)
            }
            Err(err) => {
                *self.cached_health.lock().await = Some(CachedTargetHealth::unhealthy(&err));
                if let DaemonClientError::Rpc { .. } | DaemonClientError::Decode(_) = &err {
                    tracing::warn!(target = %name, error = %err, "target health refresh failed");
                } else {
                    tracing::warn!(target = %name, error = %err, "target unreachable during health refresh");
                }
                Err(err.into())
            }
        }
    }
}

impl RemoteTargetHandle<'_> {
    pub(crate) async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.handle.cached_daemon_info().await
    }

    pub(crate) async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferExportResponse, DaemonClientError> {
        self.client.transfer_export_to_file(req, archive_path).await
    }

    pub(crate) async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        self.client.transfer_export_stream(req).await
    }

    pub(crate) async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        self.client.transfer_path_info(req).await
    }

    pub(crate) async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        self.client
            .transfer_import_from_file(archive_path, req)
            .await
    }

    pub(crate) async fn transfer_import_from_archive_stream<S, E>(
        &self,
        req: &TransferImportRequest,
        stream: S,
    ) -> Result<TransferImportResponse, DaemonClientError>
    where
        S: futures_util::TryStream<Ok = bytes::Bytes, Error = E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.client
            .transfer_import_from_archive_stream(req, stream)
            .await
    }
}
