use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, FileEditRequest, FileEditResponse, FileReadRequest,
    FileReadResponse, FileWriteRequest, FileWriteResponse, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse,
};

use crate::daemon_client::{DaemonClientError, RpcToolErrorMode, normalize_tool_result};

use super::TargetHandle;

impl TargetHandle {
    async fn normalize_checked_result<T>(
        &self,
        rpc_mode: RpcToolErrorMode,
        result: Result<T, DaemonClientError>,
    ) -> anyhow::Result<T> {
        normalize_tool_result(result, rpc_mode)
    }

    pub async fn exec_start_checked(
        &self,
        target_name: &str,
        req: &ExecStartRequest,
    ) -> anyhow::Result<ExecResponse> {
        self.ensure_daemon_info_cached(target_name).await?;
        self.normalize_checked_result(RpcToolErrorMode::Full, self.exec_start(req).await)
            .await
    }

    pub async fn patch_apply_checked(
        &self,
        target_name: &str,
        req: &PatchApplyRequest,
    ) -> anyhow::Result<PatchApplyResponse> {
        self.ensure_daemon_info_cached(target_name).await?;
        self.normalize_checked_result(RpcToolErrorMode::Full, self.patch_apply(req).await)
            .await
    }

    pub async fn image_read_checked(
        &self,
        target_name: &str,
        req: &ImageReadRequest,
    ) -> anyhow::Result<ImageReadResponse> {
        self.ensure_daemon_info_cached(target_name).await?;
        self.normalize_checked_result(RpcToolErrorMode::MessageOnly, self.image_read(req).await)
            .await
    }

    pub async fn file_read_checked(
        &self,
        target_name: &str,
        req: &FileReadRequest,
    ) -> anyhow::Result<FileReadResponse> {
        self.ensure_daemon_info_cached(target_name).await?;
        self.normalize_checked_result(RpcToolErrorMode::Full, self.file_read(req).await)
            .await
    }

    pub async fn file_write_checked(
        &self,
        target_name: &str,
        req: &FileWriteRequest,
    ) -> anyhow::Result<FileWriteResponse> {
        self.ensure_daemon_info_cached(target_name).await?;
        self.normalize_checked_result(RpcToolErrorMode::Full, self.file_write(req).await)
            .await
    }

    pub async fn file_edit_checked(
        &self,
        target_name: &str,
        req: &FileEditRequest,
    ) -> anyhow::Result<FileEditResponse> {
        self.ensure_daemon_info_cached(target_name).await?;
        self.normalize_checked_result(RpcToolErrorMode::Full, self.file_edit(req).await)
            .await
    }
}
