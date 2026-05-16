use std::sync::Arc;

use remote_exec_proto::rpc::{
    DaemonIdentity, ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, TargetCapabilities,
    TargetInfoResponse, TransferExportRequest, TransferImportRequest, TransferImportResponse,
    TransferPathInfoRequest, TransferPathInfoResponse,
};
use tokio::sync::Mutex;

use crate::daemon_client::{DaemonClientError, TransferExportResponse, TransferExportStream};

use super::{TargetBackend, ensure_expected_daemon_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub identity: DaemonIdentity,
    pub capabilities: TargetCapabilities,
    pub supports_transfer_compression: bool,
}

#[derive(Clone)]
pub struct TargetHandle {
    pub(super) backend: TargetBackend,
    expected_daemon_name: Option<String>,
    identity_verified: Arc<Mutex<bool>>,
    cached_daemon_info: Arc<Mutex<Option<CachedDaemonInfo>>>,
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

    pub(crate) fn verified(
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

    pub(crate) fn unavailable(
        backend: TargetBackend,
        expected_daemon_name: Option<String>,
    ) -> Self {
        Self::new(backend, expected_daemon_name, false, None)
    }

    fn cache_from_target_info(info: &TargetInfoResponse) -> CachedDaemonInfo {
        CachedDaemonInfo {
            identity: info.identity.clone(),
            capabilities: info.capabilities.clone(),
            supports_transfer_compression: info.supports_transfer_compression,
        }
    }

    pub(crate) async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.cached_daemon_info.lock().await.clone()
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

    pub(crate) fn as_remote(&self) -> Option<RemoteTargetHandle<'_>> {
        self.backend.remote_client().map(|client| RemoteTargetHandle {
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

    pub(crate) async fn clear_cached_daemon_info(&self) {
        *self.identity_verified.lock().await = false;
        *self.cached_daemon_info.lock().await = None;
        tracing::info!("cleared cached daemon identity and metadata");
    }

    pub(crate) async fn ensure_identity_verified(&self, name: &str) -> anyhow::Result<()> {
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
            platform = %info.identity.platform,
            arch = %info.identity.arch,
            hostname = %info.identity.hostname,
            supports_pty = info.capabilities.supports_pty,
            "verified target identity"
        );
        Ok(())
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

    pub(crate) async fn transfer_import_from_body(
        &self,
        req: &TransferImportRequest,
        body: reqwest::Body,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        self.client.transfer_import_from_body(req, body).await
    }

    pub(crate) async fn clear_on_transport_error<T>(
        &self,
        result: Result<T, DaemonClientError>,
    ) -> Result<T, DaemonClientError> {
        self.handle.clear_on_transport_error(result).await
    }
}
