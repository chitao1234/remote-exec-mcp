use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};

#[derive(Clone)]
pub(crate) enum TargetBackend {
    Remote(crate::daemon_client::DaemonClient),
    Local(crate::local_backend::LocalDaemonClient),
}

impl TargetBackend {
    pub(crate) async fn target_info(
        &self,
    ) -> Result<TargetInfoResponse, crate::daemon_client::DaemonClientError> {
        match self {
            Self::Remote(client) => client.target_info().await,
            Self::Local(client) => client.target_info().await,
        }
    }

    pub(crate) async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, crate::daemon_client::DaemonClientError> {
        match self {
            Self::Remote(client) => client.exec_start(req).await,
            Self::Local(client) => client.exec_start(req).await,
        }
    }

    pub(crate) async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, crate::daemon_client::DaemonClientError> {
        match self {
            Self::Remote(client) => client.exec_write(req).await,
            Self::Local(client) => client.exec_write(req).await,
        }
    }

    pub(crate) async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, crate::daemon_client::DaemonClientError> {
        match self {
            Self::Remote(client) => client.patch_apply(req).await,
            Self::Local(client) => client.patch_apply(req).await,
        }
    }

    pub(crate) async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, crate::daemon_client::DaemonClientError> {
        match self {
            Self::Remote(client) => client.image_read(req).await,
            Self::Local(client) => client.image_read(req).await,
        }
    }

    pub(crate) fn remote_client(&self) -> Option<&crate::daemon_client::DaemonClient> {
        match self {
            Self::Remote(client) => Some(client),
            Self::Local(_) => None,
        }
    }

    pub(crate) async fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> Result<crate::port_forward::PortTunnel, crate::daemon_client::DaemonClientError> {
        match self {
            Self::Remote(client) => {
                crate::port_forward::PortTunnel::from_stream_with_max_queued_bytes(
                    client.port_tunnel().await?,
                    max_queued_bytes,
                )
            }
            Self::Local(client) => {
                crate::port_forward::PortTunnel::local(client.port_tunnel_state(), max_queued_bytes)
                    .await
            }
        }
    }
}
