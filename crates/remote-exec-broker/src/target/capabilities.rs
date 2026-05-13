use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ImageReadRequest, ImageReadResponse, PatchApplyRequest,
    PatchApplyResponse,
};

use crate::daemon_client::{DaemonClientError, RpcToolErrorMode, normalize_tool_result};

use super::TargetHandle;

impl TargetHandle {
    async fn checked_call<T>(
        &self,
        target_name: &str,
        rpc_mode: RpcToolErrorMode,
        result: Result<T, DaemonClientError>,
    ) -> anyhow::Result<T> {
        self.ensure_identity_verified(target_name).await?;
        normalize_tool_result(self.clear_on_transport_error(result).await, rpc_mode)
    }

    pub async fn clear_on_transport_error<T>(
        &self,
        result: Result<T, DaemonClientError>,
    ) -> Result<T, DaemonClientError> {
        if matches!(result, Err(DaemonClientError::Transport(_))) {
            self.clear_cached_daemon_info().await;
        }
        result
    }

    pub async fn exec_start_checked(
        &self,
        target_name: &str,
        req: &ExecStartRequest,
    ) -> anyhow::Result<ExecResponse> {
        self.checked_call(
            target_name,
            RpcToolErrorMode::Full,
            self.exec_start(req).await,
        )
        .await
    }

    pub async fn patch_apply_checked(
        &self,
        target_name: &str,
        req: &PatchApplyRequest,
    ) -> anyhow::Result<PatchApplyResponse> {
        self.checked_call(
            target_name,
            RpcToolErrorMode::Full,
            self.patch_apply(req).await,
        )
        .await
    }

    pub async fn image_read_checked(
        &self,
        target_name: &str,
        req: &ImageReadRequest,
    ) -> anyhow::Result<ImageReadResponse> {
        self.checked_call(
            target_name,
            RpcToolErrorMode::MessageOnly,
            self.image_read(req).await,
        )
        .await
    }
}
