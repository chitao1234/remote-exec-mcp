use std::sync::Arc;

use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};

use crate::daemon_client::{DaemonClientError, DaemonRpcCode};

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
        let host_config = config.host_runtime_config(sandbox, enable_transfer_compression);
        let state = remote_exec_host::build_runtime_state(host_config)?;
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
}

pub(crate) fn map_local_image_error(err: remote_exec_host::ImageError) -> DaemonClientError {
    map_host_rpc_error(err.into())
}

pub(crate) fn map_local_transfer_error(err: remote_exec_host::TransferError) -> DaemonClientError {
    map_host_rpc_error(err.into())
}

pub(crate) fn map_host_rpc_error(err: remote_exec_host::HostRpcError) -> DaemonClientError {
    let (status, body) = err.into_http_rpc_parts("broker_local");
    DaemonClientError::Rpc {
        status: reqwest::StatusCode::from_u16(status)
            .expect("normalized HostRpcError status is valid"),
        code: Some(DaemonRpcCode::from_wire_value(body.code)),
        message: body.message,
    }
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::RpcErrorCode;

    use super::DaemonClientError;

    #[test]
    fn host_internal_errors_preserve_server_status_for_local_backend() {
        let err = super::map_host_rpc_error(remote_exec_host::HostRpcError {
            status: 500,
            code: RpcErrorCode::Internal,
            message: "boom".to_string(),
        });

        assert!(matches!(
            &err,
            DaemonClientError::Rpc {
                status,
                message,
                ..
            } if *status == reqwest::StatusCode::INTERNAL_SERVER_ERROR && message == "boom"
        ));
        assert_eq!(err.rpc_code(), Some("internal_error"));
        assert_eq!(err.rpc_error_code(), Some(RpcErrorCode::Internal));
    }

    #[test]
    fn invalid_host_status_falls_back_to_internal_server_error() {
        let err = super::map_host_rpc_error(remote_exec_host::HostRpcError {
            status: 42,
            code: RpcErrorCode::Internal,
            message: "invalid status".to_string(),
        });

        assert!(matches!(
            &err,
            DaemonClientError::Rpc {
                status,
                message,
                ..
            } if *status == reqwest::StatusCode::INTERNAL_SERVER_ERROR && message == "invalid status"
        ));
        assert_eq!(err.rpc_code(), Some("internal_error"));
        assert_eq!(err.rpc_error_code(), Some(RpcErrorCode::Internal));
    }
}
