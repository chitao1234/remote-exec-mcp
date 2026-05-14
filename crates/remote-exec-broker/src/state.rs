use std::collections::BTreeMap;

use anyhow::Context;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::transfer::TransferLimits;

use crate::{port_forward, session_store::SessionStore, target::TargetHandle};

pub(crate) const LOCAL_TARGET_NAME: &str = "local";

#[derive(Clone)]
pub struct BrokerState {
    pub(crate) enable_transfer_compression: bool,
    pub(crate) transfer_limits: TransferLimits,
    pub(crate) disable_structured_content: bool,
    pub(crate) port_forward_limits: port_forward::BrokerPortForwardLimits,
    pub(crate) host_sandbox: Option<CompiledFilesystemSandbox>,
    pub(crate) sessions: SessionStore,
    pub(crate) port_forwards: port_forward::PortForwardStore,
    pub(crate) targets: BTreeMap<String, TargetHandle>,
}

impl BrokerState {
    pub fn target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .with_context(|| format!("unknown target `{name}`"))
    }

    pub async fn forwarding_side(&self, name: &str) -> anyhow::Result<port_forward::SideHandle> {
        if name == LOCAL_TARGET_NAME && !self.targets.contains_key(LOCAL_TARGET_NAME) {
            return port_forward::SideHandle::local();
        }

        let handle = self.target(name)?;
        handle.ensure_identity_verified(name).await?;
        if let Some(info) = handle.cached_daemon_info().await {
            anyhow::ensure!(
                info.supports_port_forward
                    && info
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
