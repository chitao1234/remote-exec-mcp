use std::sync::Arc;

use remote_exec_proto::rpc::{
    EmptyResponse, ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, PortConnectRequest,
    PortConnectResponse, PortConnectionCloseRequest, PortConnectionReadRequest,
    PortConnectionReadResponse, PortConnectionWriteRequest, PortLeaseRenewRequest,
    PortListenAcceptRequest, PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest,
    PortListenResponse, PortUdpDatagramReadRequest, PortUdpDatagramReadResponse,
    PortUdpDatagramWriteRequest, RpcErrorBody, TargetInfoResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};

use crate::daemon_client::DaemonClientError;

#[derive(Clone)]
pub struct LocalDaemonClient {
    state: Arc<remote_exec_host::HostRuntimeState>,
}

impl LocalDaemonClient {
    pub fn new(
        config: &crate::config::LocalTargetConfig,
        sandbox: Option<remote_exec_proto::sandbox::FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> anyhow::Result<Self> {
        let embedded = config.embedded_host_config(sandbox, enable_transfer_compression);
        let state = remote_exec_host::build_runtime_state(embedded.into_host_runtime_config())?;
        Ok(Self {
            state: Arc::new(state),
        })
    }

    pub async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        Ok(remote_exec_host::target_info_response(
            &self.state,
            env!("CARGO_PKG_VERSION"),
        ))
    }

    pub fn port_tunnel_state(&self) -> Arc<remote_exec_host::HostRuntimeState> {
        self.state.clone()
    }

    pub async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        remote_exec_host::exec::exec_start_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        remote_exec_host::exec::exec_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        remote_exec_host::patch::apply_patch_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        remote_exec_host::image::read_image_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_image_error)
    }

    pub async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        remote_exec_host::transfer::path_info_for_request(&self.state, req)
            .map_err(map_local_transfer_error)
    }

    pub async fn port_listen(
        &self,
        req: &PortListenRequest,
    ) -> Result<PortListenResponse, DaemonClientError> {
        remote_exec_host::port_forward::listen_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_listen_accept(
        &self,
        req: &PortListenAcceptRequest,
    ) -> Result<PortListenAcceptResponse, DaemonClientError> {
        remote_exec_host::port_forward::listen_accept_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_listen_close(
        &self,
        req: &PortListenCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::listen_close_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_lease_renew(
        &self,
        req: &PortLeaseRenewRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::lease_renew_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connect(
        &self,
        req: &PortConnectRequest,
    ) -> Result<PortConnectResponse, DaemonClientError> {
        remote_exec_host::port_forward::connect_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connection_read(
        &self,
        req: &PortConnectionReadRequest,
    ) -> Result<PortConnectionReadResponse, DaemonClientError> {
        remote_exec_host::port_forward::connection_read_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connection_write(
        &self,
        req: &PortConnectionWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::connection_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connection_close(
        &self,
        req: &PortConnectionCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::connection_close_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_udp_datagram_read(
        &self,
        req: &PortUdpDatagramReadRequest,
    ) -> Result<PortUdpDatagramReadResponse, DaemonClientError> {
        remote_exec_host::port_forward::udp_datagram_read_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_udp_datagram_write(
        &self,
        req: &PortUdpDatagramWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::udp_datagram_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }
}

pub(crate) fn map_local_image_error(err: remote_exec_host::ImageError) -> DaemonClientError {
    map_host_rpc_error(err.into_host_rpc_error())
}

pub(crate) fn map_local_transfer_error(err: remote_exec_host::TransferError) -> DaemonClientError {
    map_host_rpc_error(err.into_host_rpc_error())
}

pub(crate) fn map_host_rpc_error(err: remote_exec_host::HostRpcError) -> DaemonClientError {
    local_rpc_error_body(
        reqwest::StatusCode::from_u16(err.status).expect("valid status code"),
        RpcErrorBody {
            code: err.code.to_string(),
            message: err.message,
        },
    )
}

fn local_rpc_error_body(status: reqwest::StatusCode, body: RpcErrorBody) -> DaemonClientError {
    DaemonClientError::Rpc {
        status,
        code: Some(body.code),
        message: body.message,
    }
}

#[cfg(test)]
mod tests {
    use super::DaemonClientError;

    #[test]
    fn host_internal_errors_preserve_server_status_for_local_backend() {
        let err = super::map_host_rpc_error(remote_exec_host::HostRpcError {
            status: 500,
            code: "internal_error",
            message: "boom".to_string(),
        });

        match err {
            DaemonClientError::Rpc {
                status,
                code,
                message,
            } => {
                assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(code.as_deref(), Some("internal_error"));
                assert_eq!(message, "boom");
            }
            other => panic!("expected rpc error, got {other:?}"),
        }
    }
}
