use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Context;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest, FileEditResponse,
    FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, RpcErrorCode,
};
use remote_exec_proto::transfer::TransferLimits;

use crate::{
    daemon_client::{RpcToolErrorMode, normalize_tool_result},
    local::{self, BrokerHostOrTarget},
    port_forward,
    session_store::{SessionRecord, SessionStore},
    target::{CachedDaemonInfo, RemoteTargetHandle, TargetHandle},
};

pub(crate) struct BrokerStateInit {
    pub(crate) enable_transfer_compression: bool,
    pub(crate) transfer_limits: TransferLimits,
    pub(crate) disable_structured_content: bool,
    pub(crate) health_refresh_interval: Duration,
    pub(crate) tools: crate::config::BrokerToolsConfig,
    pub(crate) port_forward_limits: port_forward::BrokerPortForwardLimits,
    pub(crate) host_sandbox: Option<CompiledFilesystemSandbox>,
    pub(crate) sessions: SessionStore,
    pub(crate) port_forwards: port_forward::PortForwardStore,
    pub(crate) targets: BTreeMap<String, TargetHandle>,
}

#[derive(Clone)]
pub struct BrokerState {
    pub(crate) enable_transfer_compression: bool,
    pub(crate) transfer_limits: TransferLimits,
    pub(crate) disable_structured_content: bool,
    pub(crate) health_refresh_interval: Duration,
    pub(crate) tools: crate::config::BrokerToolsConfig,
    pub(crate) port_forward_limits: port_forward::BrokerPortForwardLimits,
    pub(crate) host_sandbox: Option<CompiledFilesystemSandbox>,
    pub(crate) sessions: SessionStore,
    pub(crate) port_forwards: port_forward::PortForwardStore,
    targets: BTreeMap<String, TargetHandle>,
}

pub(crate) struct TargetStatusSnapshot {
    pub(crate) name: String,
    pub(crate) daemon_info: Option<CachedDaemonInfo>,
}

pub(crate) struct RegisteredExecSession {
    pub(crate) public_session_id: String,
}

pub(crate) struct ExecSessionWrite {
    pub(crate) record: SessionRecord,
    pub(crate) response: ExecResponse,
    pub(crate) public_session_id: Option<String>,
}

impl BrokerState {
    pub(crate) fn new(init: BrokerStateInit) -> Self {
        Self {
            enable_transfer_compression: init.enable_transfer_compression,
            transfer_limits: init.transfer_limits,
            disable_structured_content: init.disable_structured_content,
            health_refresh_interval: init.health_refresh_interval,
            tools: init.tools,
            port_forward_limits: init.port_forward_limits,
            host_sandbox: init.host_sandbox,
            sessions: init.sessions,
            port_forwards: init.port_forwards,
            targets: init.targets,
        }
    }

    pub(crate) fn configured_target_count(&self) -> usize {
        self.targets.len()
    }

    pub(crate) async fn target_status_snapshots(&self) -> Vec<TargetStatusSnapshot> {
        let mut snapshots = Vec::with_capacity(self.targets.len());
        for (name, handle) in &self.targets {
            snapshots.push(TargetStatusSnapshot {
                name: name.clone(),
                daemon_info: handle.cached_daemon_info().await,
            });
        }
        snapshots
    }

    pub(crate) fn remote_target_names(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter(|(_, handle)| handle.as_remote().is_some())
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub(crate) async fn refresh_remote_target_health(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let handle = self.configured_target(name)?;
        anyhow::ensure!(
            handle.as_remote().is_some(),
            "target `{name}` is not a remote target"
        );
        handle.refresh_health_and_cache(name).await
    }

    fn configured_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .with_context(|| format!("unknown target `{name}`"))
    }

    async fn verified_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        let handle = self.configured_target(name)?;
        handle.ensure_daemon_info_cached(name).await?;
        Ok(handle)
    }

    pub(crate) async fn exec_path_policy(&self, name: &str) -> anyhow::Result<PathPolicy> {
        let target = self.verified_target(name).await?;
        let info = target.cached_daemon_info_after_verification(name).await?;
        Ok(info.path_policy())
    }

    pub(crate) async fn exec_start(
        &self,
        name: &str,
        req: &ExecStartRequest,
    ) -> anyhow::Result<ExecResponse> {
        let target = self.verified_target(name).await?;
        normalize_tool_result(target.exec_start(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn patch_apply(
        &self,
        name: &str,
        req: &PatchApplyRequest,
    ) -> anyhow::Result<PatchApplyResponse> {
        let target = self.verified_target(name).await?;
        normalize_tool_result(target.patch_apply(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn image_read(
        &self,
        name: &str,
        req: &ImageReadRequest,
    ) -> anyhow::Result<ImageReadResponse> {
        let target = self.verified_target(name).await?;
        normalize_tool_result(target.image_read(req).await, RpcToolErrorMode::MessageOnly)
    }

    async fn session_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.verified_target(name).await
    }

    pub(crate) async fn register_exec_session(
        &self,
        target: &str,
        daemon_session_id: String,
        daemon_instance_id: String,
        session_command: String,
    ) -> RegisteredExecSession {
        let record = self
            .sessions
            .insert(
                target.to_string(),
                daemon_session_id,
                daemon_instance_id,
                session_command,
            )
            .await;
        RegisteredExecSession {
            public_session_id: record.session_id,
        }
    }

    pub(crate) async fn write_exec_session(
        &self,
        public_session_id: &str,
        requested_target: Option<&str>,
        chars: String,
        yield_time_ms: Option<u64>,
        max_output_tokens: Option<u32>,
        pty_size: Option<remote_exec_proto::rpc::ExecPtySize>,
    ) -> anyhow::Result<ExecSessionWrite> {
        let record = self
            .sessions
            .get(public_session_id)
            .await
            .with_context(|| unknown_process_id_message(public_session_id))?;
        crate::request_context::set_current_target(record.target.clone());

        if let Some(target) = requested_target {
            anyhow::ensure!(
                target == record.target,
                "session does not belong to target `{target}`"
            );
        }

        let target = self.session_target(&record.target).await?;
        let request = ExecWriteRequest {
            daemon_session_id: record.daemon_session_id.clone(),
            chars,
            yield_time_ms,
            max_output_tokens,
            pty_size,
        };
        let response = target.exec_write(&request).await;
        let response = match response {
            Ok(response) => response,
            Err(err) if err.is_rpc_error_code(RpcErrorCode::UnknownSession) => {
                self.sessions.remove(&record.session_id).await;
                return Err(anyhow::anyhow!(unknown_process_id_message(
                    &record.session_id
                )));
            }
            Err(err) => {
                if let Ok(info) = target.target_info().await {
                    if info.daemon_instance_id != record.daemon_instance_id {
                        target.invalidate_cached_daemon_info().await;
                        self.sessions.remove(&record.session_id).await;
                        return Err(anyhow::anyhow!(unknown_process_id_message(
                            &record.session_id
                        )));
                    }
                }
                return normalize_tool_result(Err(err), RpcToolErrorMode::Full);
            }
        };

        let public_session_id = if response.running() {
            Some(record.session_id.clone())
        } else {
            self.sessions.remove(&record.session_id).await;
            None
        };

        Ok(ExecSessionWrite {
            record,
            response,
            public_session_id,
        })
    }

    async fn file_tool_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        let target = self.verified_target(name).await?;
        let info = target.cached_daemon_info_after_verification(name).await?;
        let Some(version) = info.capabilities.file_tool_protocol_version else {
            anyhow::bail!("target `{name}` does not support file tool protocol version 1");
        };
        anyhow::ensure!(
            version.get() >= 1,
            "target `{name}` does not support file tool protocol version 1"
        );
        Ok(target)
    }

    pub(crate) async fn file_read(
        &self,
        name: &str,
        req: &FileReadRequest,
    ) -> anyhow::Result<FileReadResponse> {
        let target = self.file_tool_target(name).await?;
        normalize_tool_result(target.file_read(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn file_write(
        &self,
        name: &str,
        req: &FileWriteRequest,
    ) -> anyhow::Result<FileWriteResponse> {
        let target = self.file_tool_target(name).await?;
        normalize_tool_result(target.file_write(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn file_edit(
        &self,
        name: &str,
        req: &FileEditRequest,
    ) -> anyhow::Result<FileEditResponse> {
        let target = self.file_tool_target(name).await?;
        normalize_tool_result(target.file_edit(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn invalidate_target_exec_sessions(&self, target: &str) {
        self.sessions.remove_target(target).await;
    }

    pub(crate) async fn transfer_remote_target<'a>(
        &'a self,
        name: &'a str,
    ) -> anyhow::Result<RemoteTargetHandle<'a>> {
        self.verified_remote_target(name).await
    }

    pub(crate) async fn transfer_remote_daemon_info(
        &self,
        name: &str,
    ) -> anyhow::Result<CachedDaemonInfo> {
        let target = self.transfer_remote_target(name).await?;
        target.cached_daemon_info_after_verification(name).await
    }

    pub(crate) fn configured_local_target_enabled(&self) -> bool {
        self.targets.contains_key(local::TARGET_NAME)
    }

    pub(crate) async fn port_forward_side(
        &self,
        name: &str,
    ) -> anyhow::Result<port_forward::SideHandle> {
        if BrokerHostOrTarget::from_name(name) == BrokerHostOrTarget::BrokerHost
            && !self.configured_local_target_enabled()
        {
            return port_forward::SideHandle::broker_host();
        }

        let handle = self.verified_target(name).await?;
        let info = handle.cached_daemon_info_after_verification(name).await?;
        anyhow::ensure!(
            info.capabilities.supports_port_forward
                && info
                    .capabilities
                    .port_forward_protocol_version
                    .is_some_and(|version| version.get() >= 4),
            "target `{name}` does not support port forward protocol version 4"
        );
        Ok(port_forward::SideHandle::target(
            name.to_string(),
            handle.clone(),
            info.daemon_instance_id,
        ))
    }

    async fn verified_remote_target<'a>(
        &'a self,
        name: &'a str,
    ) -> anyhow::Result<RemoteTargetHandle<'a>> {
        let target = self.verified_target(name).await?;
        target
            .as_remote()
            .with_context(|| format!("target `{name}` is not a remote target"))
    }
}

pub(crate) fn unknown_process_id_message(session_id: &str) -> String {
    format!("Unknown process id {session_id}")
}
