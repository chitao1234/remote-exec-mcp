use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use futures_util::future::join_all;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest, FileEditResponse,
    FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, RpcErrorCode,
};
use remote_exec_proto::transfer::TransferLimits;

use crate::{
    daemon_client::{DaemonClientError, RpcToolErrorMode, normalize_tool_result},
    local::{self, BrokerHostOrTarget},
    port_forward,
    session_store::{SessionRecord, SessionStore},
    target::{CachedDaemonInfo, RemoteTargetHandle, TargetHandle},
};

pub(crate) struct BrokerStateInit {
    pub(crate) enable_transfer_compression: bool,
    pub(crate) transfer_limits: TransferLimits,
    pub(crate) disable_structured_content: bool,
    pub(crate) health_refresh_intervals: TargetHealthRefreshIntervals,
    pub(crate) tools: crate::config::BrokerToolsConfig,
    pub(crate) port_forward_limits: port_forward::BrokerPortForwardLimits,
    pub(crate) host_sandbox: Option<CompiledFilesystemSandbox>,
    pub(crate) host_filesystem: BrokerHostFilesystemConfig,
    pub(crate) sessions: SessionStore,
    pub(crate) port_forwards: port_forward::PortForwardStore,
    pub(crate) targets: BTreeMap<String, TargetHandle>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BrokerHostFilesystemConfig {
    windows_posix_root: Option<PathBuf>,
}

impl BrokerHostFilesystemConfig {
    pub(crate) fn from_local_config(local: Option<&crate::config::LocalTargetConfig>) -> Self {
        Self {
            windows_posix_root: local.and_then(|config| config.windows_posix_root.clone()),
        }
    }

    pub(crate) fn windows_posix_root(&self) -> Option<&Path> {
        self.windows_posix_root.as_deref()
    }
}

#[derive(Clone)]
pub struct BrokerState {
    pub(crate) enable_transfer_compression: bool,
    pub(crate) transfer_limits: TransferLimits,
    pub(crate) disable_structured_content: bool,
    pub(crate) health_refresh_intervals: TargetHealthRefreshIntervals,
    pub(crate) tools: crate::config::BrokerToolsConfig,
    pub(crate) port_forward_limits: port_forward::BrokerPortForwardLimits,
    pub(crate) host_sandbox: Option<CompiledFilesystemSandbox>,
    pub(crate) host_filesystem: BrokerHostFilesystemConfig,
    pub(crate) sessions: SessionStore,
    pub(crate) port_forwards: port_forward::PortForwardStore,
    targets: BTreeMap<String, TargetHandle>,
}

pub(crate) struct TargetStatusSnapshot {
    pub(crate) name: String,
    pub(crate) healthy: bool,
    pub(crate) daemon_info: Option<CachedDaemonInfo>,
}

const TARGET_STATUS_RECHECK_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
pub(crate) struct TargetHealthRefreshIntervals {
    pub(crate) healthy: Duration,
    pub(crate) unhealthy: Duration,
}

impl TargetHealthRefreshIntervals {
    pub(crate) fn shortest(self) -> Duration {
        self.healthy.min(self.unhealthy)
    }

    pub(crate) fn is_due(
        self,
        health: Option<&crate::target::CachedTargetHealth>,
        now: SystemTime,
    ) -> bool {
        let Some(health) = health else {
            return true;
        };

        let interval = if health.healthy {
            self.healthy
        } else {
            self.unhealthy
        };
        now.duration_since(health.last_checked_at)
            .map(|elapsed| elapsed >= interval)
            .unwrap_or(false)
    }
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
            health_refresh_intervals: init.health_refresh_intervals,
            tools: init.tools,
            port_forward_limits: init.port_forward_limits,
            host_sandbox: init.host_sandbox,
            host_filesystem: init.host_filesystem,
            sessions: init.sessions,
            port_forwards: init.port_forwards,
            targets: init.targets,
        }
    }

    pub(crate) fn configured_target_count(&self) -> usize {
        self.targets.len()
    }

    pub(crate) async fn target_status_snapshots(&self) -> Vec<TargetStatusSnapshot> {
        self.refresh_status_targets().await;

        let mut snapshots = Vec::with_capacity(self.targets.len());
        for (name, handle) in &self.targets {
            let status = handle.cached_status().await;
            snapshots.push(TargetStatusSnapshot {
                name: name.clone(),
                healthy: status.healthy,
                daemon_info: status.daemon_info,
            });
        }
        snapshots
    }

    async fn refresh_status_targets(&self) {
        let mut names = Vec::new();
        for (name, handle) in &self.targets {
            if handle.as_remote().is_some() && handle.needs_status_recheck().await {
                names.push(name.clone());
            }
        }

        let refreshes = names.into_iter().map(|name| {
            let state = self.clone();
            async move {
                match tokio::time::timeout(
                    TARGET_STATUS_RECHECK_TIMEOUT,
                    state.refresh_remote_target_health_and_dependents(&name),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        tracing::debug!(
                            target = %name,
                            error = %err,
                            "target status recheck did not update cached daemon metadata"
                        );
                    }
                    Err(_) => {
                        tracing::debug!(
                            target = %name,
                            timeout_ms = TARGET_STATUS_RECHECK_TIMEOUT.as_millis(),
                            "target status recheck timed out"
                        );
                    }
                }
            }
        });
        join_all(refreshes).await;
    }

    pub(crate) async fn remote_targets_due_for_health_refresh(
        &self,
        now: SystemTime,
    ) -> Vec<String> {
        let mut names = Vec::new();
        for (name, handle) in &self.targets {
            if handle.as_remote().is_none() {
                continue;
            }
            let health = handle.cached_health().await;
            if self.health_refresh_intervals.is_due(health.as_ref(), now) {
                names.push(name.clone());
            }
        }
        names
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

    pub(crate) async fn refresh_remote_target_health_and_dependents(
        &self,
        name: &str,
    ) -> anyhow::Result<()> {
        match self.refresh_remote_target_health(name).await {
            Ok(Some(previous_daemon_instance_id)) => {
                self.invalidate_target_exec_sessions(name).await;
                match self
                    .port_forwards
                    .close_target_instance(
                        name,
                        &previous_daemon_instance_id,
                        "target daemon instance changed",
                    )
                    .await
                {
                    Ok(closed) if !closed.is_empty() => {
                        tracing::info!(
                            target = %name,
                            previous_daemon_instance_id = %previous_daemon_instance_id,
                            closed_forwards = closed.len(),
                            "closed broker port forwards after daemon instance change"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(
                            target = %name,
                            previous_daemon_instance_id = %previous_daemon_instance_id,
                            error = %err,
                            "failed to close broker port forwards after daemon instance change"
                        );
                    }
                }
                tracing::info!(
                    target = %name,
                    "invalidated broker sessions after daemon instance change"
                );
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub(crate) fn trigger_remote_target_health_recheck(&self, name: &str) {
        let Ok(handle) = self.configured_target(name) else {
            return;
        };
        if handle.as_remote().is_none() {
            return;
        }

        let state = self.clone();
        let name = name.to_string();
        tokio::spawn(async move {
            if let Err(err) = state
                .refresh_remote_target_health_and_dependents(&name)
                .await
            {
                tracing::debug!(
                    target = %name,
                    error = %err,
                    "triggered target health recheck did not update cached daemon metadata"
                );
            }
        });
    }

    pub(crate) fn trigger_remote_target_health_recheck_if_daemon_error(
        &self,
        name: &str,
        err: &anyhow::Error,
    ) {
        if err
            .chain()
            .any(|cause| cause.downcast_ref::<DaemonClientError>().is_some())
        {
            self.trigger_remote_target_health_recheck(name);
        }
    }

    fn configured_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .with_context(|| format!("unknown target `{name}`"))
    }

    async fn verified_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        let handle = self.configured_target(name)?;
        if let Err(err) = handle.ensure_daemon_info_cached(name).await {
            self.trigger_remote_target_health_recheck(name);
            return Err(err);
        }
        Ok(handle)
    }

    fn normalize_target_result<T>(
        &self,
        name: &str,
        result: Result<T, DaemonClientError>,
        mode: RpcToolErrorMode,
    ) -> anyhow::Result<T> {
        if result.is_err() {
            self.trigger_remote_target_health_recheck(name);
        }
        normalize_tool_result(result, mode)
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
        self.normalize_target_result(name, target.exec_start(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn patch_apply(
        &self,
        name: &str,
        req: &PatchApplyRequest,
    ) -> anyhow::Result<PatchApplyResponse> {
        let target = self.verified_target(name).await?;
        self.normalize_target_result(name, target.patch_apply(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn image_read(
        &self,
        name: &str,
        req: &ImageReadRequest,
    ) -> anyhow::Result<ImageReadResponse> {
        let target = self.verified_target(name).await?;
        self.normalize_target_result(
            name,
            target.image_read(req).await,
            RpcToolErrorMode::MessageOnly,
        )
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
                self.trigger_remote_target_health_recheck(&record.target);
                self.sessions.remove(&record.session_id).await;
                return Err(anyhow::anyhow!(unknown_process_id_message(
                    &record.session_id
                )));
            }
            Err(err) => {
                self.trigger_remote_target_health_recheck(&record.target);
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
        self.normalize_target_result(name, target.file_read(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn file_write(
        &self,
        name: &str,
        req: &FileWriteRequest,
    ) -> anyhow::Result<FileWriteResponse> {
        let target = self.file_tool_target(name).await?;
        self.normalize_target_result(name, target.file_write(req).await, RpcToolErrorMode::Full)
    }

    pub(crate) async fn file_edit(
        &self,
        name: &str,
        req: &FileEditRequest,
    ) -> anyhow::Result<FileEditResponse> {
        let target = self.file_tool_target(name).await?;
        self.normalize_target_result(name, target.file_edit(req).await, RpcToolErrorMode::Full)
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
