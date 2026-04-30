use std::sync::Arc;

use axum::Json;
use remote_exec_proto::rpc::{
    EmptyResponse, ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, PortConnectRequest,
    PortConnectResponse, PortConnectionCloseRequest, PortConnectionReadRequest,
    PortConnectionReadResponse, PortConnectionWriteRequest, PortListenAcceptRequest,
    PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest, PortListenResponse,
    PortUdpDatagramReadRequest, PortUdpDatagramReadResponse, PortUdpDatagramWriteRequest,
    RpcErrorBody, TargetInfoResponse,
};

use crate::daemon_client::DaemonClientError;

#[derive(Clone)]
pub struct LocalDaemonClient {
    state: Arc<remote_exec_daemon::AppState>,
}

impl LocalDaemonClient {
    pub fn new(
        config: &crate::config::LocalTargetConfig,
        sandbox: Option<remote_exec_proto::sandbox::FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> anyhow::Result<Self> {
        let embedded = config.embedded_daemon_config(sandbox, enable_transfer_compression);
        let state = remote_exec_daemon::build_app_state(embedded.into_daemon_config())?;
        Ok(Self {
            state: Arc::new(state),
        })
    }

    pub async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        Ok(remote_exec_daemon::target_info_response(&self.state))
    }

    pub async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        remote_exec_daemon::exec::exec_start_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        remote_exec_daemon::exec::exec_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        remote_exec_daemon::patch::apply_patch_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        remote_exec_daemon::image::read_image_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_listen(
        &self,
        req: &PortListenRequest,
    ) -> Result<PortListenResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::listen_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_listen_accept(
        &self,
        req: &PortListenAcceptRequest,
    ) -> Result<PortListenAcceptResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::listen_accept_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_listen_close(
        &self,
        req: &PortListenCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::listen_close_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_connect(
        &self,
        req: &PortConnectRequest,
    ) -> Result<PortConnectResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::connect_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_connection_read(
        &self,
        req: &PortConnectionReadRequest,
    ) -> Result<PortConnectionReadResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::connection_read_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_connection_write(
        &self,
        req: &PortConnectionWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::connection_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_connection_close(
        &self,
        req: &PortConnectionCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::connection_close_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_udp_datagram_read(
        &self,
        req: &PortUdpDatagramReadRequest,
    ) -> Result<PortUdpDatagramReadResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::udp_datagram_read_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }

    pub async fn port_udp_datagram_write(
        &self,
        req: &PortUdpDatagramWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_daemon::port_forward::udp_datagram_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_local_rpc_error)
    }
}

fn map_local_rpc_error(
    (status, Json(body)): (axum::http::StatusCode, Json<RpcErrorBody>),
) -> DaemonClientError {
    DaemonClientError::Rpc {
        status: reqwest::StatusCode::from_u16(status.as_u16()).expect("valid status code"),
        code: Some(body.code),
        message: body.message,
    }
}
