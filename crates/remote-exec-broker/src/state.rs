use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Context;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::transfer::TransferLimits;

use crate::{
    local::{self, BrokerHostOrTarget},
    port_forward,
    session_store::SessionStore,
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

    pub(crate) async fn exec_target(
        &self,
        name: &str,
    ) -> anyhow::Result<(&TargetHandle, PathPolicy)> {
        let target = self.verified_target(name).await?;
        let info = target.cached_daemon_info_after_verification(name).await?;
        Ok((target, info.path_policy()))
    }

    pub(crate) async fn patch_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.verified_target(name).await
    }

    pub(crate) async fn image_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.verified_target(name).await
    }

    pub(crate) async fn session_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.verified_target(name).await
    }

    pub(crate) async fn file_tool_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
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
