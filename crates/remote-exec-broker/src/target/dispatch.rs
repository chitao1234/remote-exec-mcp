use std::sync::Arc;

use futures_util::future::BoxFuture;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest, FileEditResponse,
    FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse, HealthCheckResponse,
    ImageReadRequest, ImageReadResponse, PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};

use crate::daemon_client::{DaemonClient, DaemonClientError};
use crate::local::backend::LocalDaemonClient;
use crate::port_forward::PortTunnel;

#[derive(Clone)]
pub(crate) struct TargetBackend {
    client: Arc<dyn TargetBackendClient>,
}

trait TargetBackendClient: Send + Sync {
    fn health(&self) -> BoxFuture<'_, Result<HealthCheckResponse, DaemonClientError>>;

    fn target_info(&self) -> BoxFuture<'_, Result<TargetInfoResponse, DaemonClientError>>;

    fn exec_start<'a>(
        &'a self,
        req: &'a ExecStartRequest,
    ) -> BoxFuture<'a, Result<ExecResponse, DaemonClientError>>;

    fn exec_write<'a>(
        &'a self,
        req: &'a ExecWriteRequest,
    ) -> BoxFuture<'a, Result<ExecResponse, DaemonClientError>>;

    fn patch_apply<'a>(
        &'a self,
        req: &'a PatchApplyRequest,
    ) -> BoxFuture<'a, Result<PatchApplyResponse, DaemonClientError>>;

    fn image_read<'a>(
        &'a self,
        req: &'a ImageReadRequest,
    ) -> BoxFuture<'a, Result<ImageReadResponse, DaemonClientError>>;

    fn file_read<'a>(
        &'a self,
        req: &'a FileReadRequest,
    ) -> BoxFuture<'a, Result<FileReadResponse, DaemonClientError>>;

    fn file_write<'a>(
        &'a self,
        req: &'a FileWriteRequest,
    ) -> BoxFuture<'a, Result<FileWriteResponse, DaemonClientError>>;

    fn file_edit<'a>(
        &'a self,
        req: &'a FileEditRequest,
    ) -> BoxFuture<'a, Result<FileEditResponse, DaemonClientError>>;

    fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> BoxFuture<'_, Result<PortTunnel, DaemonClientError>>;

    fn remote_client(&self) -> Option<&DaemonClient> {
        None
    }
}

impl TargetBackend {
    pub(crate) fn remote(client: DaemonClient) -> Self {
        Self::new(client)
    }

    pub(crate) fn local(client: LocalDaemonClient) -> Self {
        Self::new(client)
    }

    fn new(client: impl TargetBackendClient + 'static) -> Self {
        Self {
            client: Arc::new(client),
        }
    }

    pub(crate) async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        self.client.target_info().await
    }

    pub(crate) async fn health(&self) -> Result<HealthCheckResponse, DaemonClientError> {
        self.client.health().await
    }

    pub(crate) async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.client.exec_start(req).await
    }

    pub(crate) async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.client.exec_write(req).await
    }

    pub(crate) async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        self.client.patch_apply(req).await
    }

    pub(crate) async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        self.client.image_read(req).await
    }

    pub(crate) async fn file_read(
        &self,
        req: &FileReadRequest,
    ) -> Result<FileReadResponse, DaemonClientError> {
        self.client.file_read(req).await
    }

    pub(crate) async fn file_write(
        &self,
        req: &FileWriteRequest,
    ) -> Result<FileWriteResponse, DaemonClientError> {
        self.client.file_write(req).await
    }

    pub(crate) async fn file_edit(
        &self,
        req: &FileEditRequest,
    ) -> Result<FileEditResponse, DaemonClientError> {
        self.client.file_edit(req).await
    }

    pub(crate) fn remote_client(&self) -> Option<&DaemonClient> {
        self.client.remote_client()
    }

    pub(crate) async fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> Result<PortTunnel, DaemonClientError> {
        self.client.port_tunnel(max_queued_bytes).await
    }
}

impl TargetBackendClient for DaemonClient {
    fn health(&self) -> BoxFuture<'_, Result<HealthCheckResponse, DaemonClientError>> {
        Box::pin(DaemonClient::health(self))
    }

    fn target_info(&self) -> BoxFuture<'_, Result<TargetInfoResponse, DaemonClientError>> {
        Box::pin(DaemonClient::target_info(self))
    }

    fn exec_start<'a>(
        &'a self,
        req: &'a ExecStartRequest,
    ) -> BoxFuture<'a, Result<ExecResponse, DaemonClientError>> {
        Box::pin(DaemonClient::exec_start(self, req))
    }

    fn exec_write<'a>(
        &'a self,
        req: &'a ExecWriteRequest,
    ) -> BoxFuture<'a, Result<ExecResponse, DaemonClientError>> {
        Box::pin(DaemonClient::exec_write(self, req))
    }

    fn patch_apply<'a>(
        &'a self,
        req: &'a PatchApplyRequest,
    ) -> BoxFuture<'a, Result<PatchApplyResponse, DaemonClientError>> {
        Box::pin(DaemonClient::patch_apply(self, req))
    }

    fn image_read<'a>(
        &'a self,
        req: &'a ImageReadRequest,
    ) -> BoxFuture<'a, Result<ImageReadResponse, DaemonClientError>> {
        Box::pin(DaemonClient::image_read(self, req))
    }

    fn file_read<'a>(
        &'a self,
        req: &'a FileReadRequest,
    ) -> BoxFuture<'a, Result<FileReadResponse, DaemonClientError>> {
        Box::pin(DaemonClient::file_read(self, req))
    }

    fn file_write<'a>(
        &'a self,
        req: &'a FileWriteRequest,
    ) -> BoxFuture<'a, Result<FileWriteResponse, DaemonClientError>> {
        Box::pin(DaemonClient::file_write(self, req))
    }

    fn file_edit<'a>(
        &'a self,
        req: &'a FileEditRequest,
    ) -> BoxFuture<'a, Result<FileEditResponse, DaemonClientError>> {
        Box::pin(DaemonClient::file_edit(self, req))
    }

    fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> BoxFuture<'_, Result<PortTunnel, DaemonClientError>> {
        Box::pin(async move {
            PortTunnel::from_stream_with_max_queued_bytes(
                DaemonClient::port_tunnel(self).await?,
                max_queued_bytes,
            )
        })
    }

    fn remote_client(&self) -> Option<&DaemonClient> {
        Some(self)
    }
}

impl TargetBackendClient for LocalDaemonClient {
    fn health(&self) -> BoxFuture<'_, Result<HealthCheckResponse, DaemonClientError>> {
        Box::pin(async move {
            let info = LocalDaemonClient::target_info(self).await?;
            Ok(HealthCheckResponse {
                status: remote_exec_proto::rpc::HealthStatus::Ok,
                daemon_version: info.identity.daemon_version,
                daemon_instance_id: info.daemon_instance_id,
            })
        })
    }

    fn target_info(&self) -> BoxFuture<'_, Result<TargetInfoResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::target_info(self))
    }

    fn exec_start<'a>(
        &'a self,
        req: &'a ExecStartRequest,
    ) -> BoxFuture<'a, Result<ExecResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::exec_start(self, req))
    }

    fn exec_write<'a>(
        &'a self,
        req: &'a ExecWriteRequest,
    ) -> BoxFuture<'a, Result<ExecResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::exec_write(self, req))
    }

    fn patch_apply<'a>(
        &'a self,
        req: &'a PatchApplyRequest,
    ) -> BoxFuture<'a, Result<PatchApplyResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::patch_apply(self, req))
    }

    fn image_read<'a>(
        &'a self,
        req: &'a ImageReadRequest,
    ) -> BoxFuture<'a, Result<ImageReadResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::image_read(self, req))
    }

    fn file_read<'a>(
        &'a self,
        req: &'a FileReadRequest,
    ) -> BoxFuture<'a, Result<FileReadResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::file_read(self, req))
    }

    fn file_write<'a>(
        &'a self,
        req: &'a FileWriteRequest,
    ) -> BoxFuture<'a, Result<FileWriteResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::file_write(self, req))
    }

    fn file_edit<'a>(
        &'a self,
        req: &'a FileEditRequest,
    ) -> BoxFuture<'a, Result<FileEditResponse, DaemonClientError>> {
        Box::pin(LocalDaemonClient::file_edit(self, req))
    }

    fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> BoxFuture<'_, Result<PortTunnel, DaemonClientError>> {
        Box::pin(async move { PortTunnel::local(self.port_tunnel_state(), max_queued_bytes).await })
    }
}
