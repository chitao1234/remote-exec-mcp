use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ImageReadRequest, ImageReadResponse, PatchApplyRequest,
    PatchApplyResponse,
};

use crate::daemon_client::DaemonClientError;

use super::TargetHandle;

impl TargetHandle {
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
        self.ensure_identity_verified(target_name).await?;
        Ok(self.clear_on_transport_error(self.exec_start(req).await).await?)
    }

    pub async fn patch_apply_checked(
        &self,
        target_name: &str,
        req: &PatchApplyRequest,
    ) -> anyhow::Result<PatchApplyResponse> {
        self.ensure_identity_verified(target_name).await?;
        Ok(self
            .clear_on_transport_error(self.patch_apply(req).await)
            .await?)
    }

    pub async fn image_read_checked(
        &self,
        target_name: &str,
        req: &ImageReadRequest,
    ) -> anyhow::Result<ImageReadResponse> {
        self.ensure_identity_verified(target_name).await?;
        Ok(self
            .clear_on_transport_error(self.image_read(req).await)
            .await?)
    }
}
