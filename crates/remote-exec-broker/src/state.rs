use std::collections::BTreeMap;

use anyhow::Context;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;

use crate::{
    local::{self, BrokerHostOrTarget},
    port_forward,
    session_store::SessionStore,
    target::{RemoteTargetHandle, TargetHandle},
};

#[derive(Clone)]
pub struct BrokerState {
    pub(crate) enable_transfer_compression: bool,
    pub(crate) transfer_limits: TransferLimits,
    pub(crate) disable_structured_content: bool,
    pub(crate) tools: crate::config::BrokerToolsConfig,
    pub(crate) port_forward_limits: port_forward::BrokerPortForwardLimits,
    pub(crate) host_sandbox: Option<CompiledFilesystemSandbox>,
    pub(crate) sessions: SessionStore,
    pub(crate) port_forwards: port_forward::PortForwardStore,
    pub(crate) targets: BTreeMap<String, TargetHandle>,
}

impl BrokerState {
    pub fn configured_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .with_context(|| format!("unknown target `{name}`"))
    }

    pub fn target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.configured_target(name)
    }

    pub async fn verified_configured_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        let handle = self.configured_target(name)?;
        handle.ensure_identity_verified(name).await?;
        Ok(handle)
    }

    pub async fn verified_target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.verified_configured_target(name).await
    }

    pub(crate) async fn verified_remote_target<'a>(
        &'a self,
        name: &'a str,
    ) -> anyhow::Result<RemoteTargetHandle<'a>> {
        let target = self.verified_configured_target(name).await?;
        target
            .as_remote()
            .with_context(|| format!("target `{name}` is not a remote target"))
    }

    pub(crate) fn configured_local_target_enabled(&self) -> bool {
        self.targets.contains_key(local::TARGET_NAME)
    }

    pub async fn forwarding_side(&self, name: &str) -> anyhow::Result<port_forward::SideHandle> {
        if BrokerHostOrTarget::from_name(name) == BrokerHostOrTarget::BrokerHost
            && !self.configured_local_target_enabled()
        {
            return port_forward::SideHandle::broker_host();
        }

        let handle = self.verified_configured_target(name).await?;
        if let Some(info) = handle.cached_daemon_info().await {
            anyhow::ensure!(
                info.capabilities.supports_port_forward
                    && info
                        .capabilities
                        .port_forward_protocol_version
                        .is_some_and(|version| version.get() >= 4),
                "target `{name}` does not support port forward protocol version 4"
            );
        }
        Ok(port_forward::SideHandle::target(
            name.to_string(),
            handle.clone(),
        ))
    }
}
