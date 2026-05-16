use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};

#[derive(Clone)]
pub(crate) enum TargetBackend {
    Remote(crate::daemon_client::DaemonClient),
    Local(crate::local::backend::LocalDaemonClient),
}

macro_rules! dispatch_rpc {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match $self {
            Self::Remote(client) => client.$method($($arg),*).await,
            Self::Local(client) => client.$method($($arg),*).await,
        }
    };
}

impl TargetBackend {
    pub(crate) async fn target_info(
        &self,
    ) -> Result<TargetInfoResponse, crate::daemon_client::DaemonClientError> {
        dispatch_rpc!(self, target_info)
    }

    pub(crate) async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, crate::daemon_client::DaemonClientError> {
        dispatch_rpc!(self, exec_start, req)
    }

    pub(crate) async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, crate::daemon_client::DaemonClientError> {
        dispatch_rpc!(self, exec_write, req)
    }

    pub(crate) async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, crate::daemon_client::DaemonClientError> {
        dispatch_rpc!(self, patch_apply, req)
    }

    pub(crate) async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, crate::daemon_client::DaemonClientError> {
        dispatch_rpc!(self, image_read, req)
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
