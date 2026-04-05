use std::sync::Arc;

use axum::Json;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, RpcErrorBody, TargetInfoResponse,
};

use crate::daemon_client::DaemonClientError;

#[derive(Clone)]
pub struct LocalDaemonClient {
    state: Arc<remote_exec_daemon::AppState>,
}

impl LocalDaemonClient {
    pub fn new(config: &crate::config::LocalTargetConfig) -> anyhow::Result<Self> {
        let embedded = config.embedded_daemon_config();
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
