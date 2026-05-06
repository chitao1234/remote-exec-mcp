use std::sync::Arc;

use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};
use tokio::sync::Mutex;

use crate::daemon_client::{DaemonClientError, TransferExportResponse, TransferExportStream};

use super::{TargetBackend, ensure_expected_daemon_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_transfer_compression: bool,
    pub supports_port_forward: bool,
    pub port_forward_protocol_version: u32,
}

#[derive(Clone)]
pub struct TargetHandle {
    pub(super) backend: TargetBackend,
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
            daemon_version: info.daemon_version.clone(),
            hostname: info.hostname.clone(),
            platform: info.platform.clone(),
            arch: info.arch.clone(),
            supports_pty: info.supports_pty,
            supports_transfer_compression: info.supports_transfer_compression,
            supports_port_forward: info.supports_port_forward,
            port_forward_protocol_version: info.port_forward_protocol_version,
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
    ) -> Result<TransferExportResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => {
                client.transfer_export_to_file(req, archive_path).await
            }
            TargetBackend::Local(_) => Err(unsupported_local_transfer_error()),
        }
    }

    pub async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.transfer_export_stream(req).await,
            TargetBackend::Local(_) => Err(unsupported_local_transfer_error()),
        }
    }

    pub async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.transfer_path_info(req).await,
            TargetBackend::Local(client) => client.transfer_path_info(req).await,
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

    pub async fn transfer_import_from_body(
        &self,
        req: &TransferImportRequest,
        body: reqwest::Body,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => client.transfer_import_from_body(req, body).await,
            TargetBackend::Local(_) => Err(unsupported_local_transfer_error()),
        }
    }

    pub async fn port_tunnel(&self) -> Result<crate::port_forward::PortTunnel, DaemonClientError> {
        match &self.backend {
            TargetBackend::Remote(client) => {
                crate::port_forward::PortTunnel::from_stream(client.port_tunnel().await?)
            }
            TargetBackend::Local(client) => {
                crate::port_forward::PortTunnel::local(client.port_tunnel_state()).await
            }
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

fn unsupported_local_transfer_error() -> DaemonClientError {
    DaemonClientError::Rpc {
        status: reqwest::StatusCode::BAD_REQUEST,
        code: Some("unsupported_operation".to_string()),
        message: "embedded local target does not use daemon transfer RPC".to_string(),
    }
}
