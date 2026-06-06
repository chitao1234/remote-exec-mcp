use crate::TargetHandle;
use crate::daemon_client::DaemonClientError;
use crate::local::TARGET_NAME;
use crate::local::port::LocalPortClient;

use super::tunnel::PortTunnel;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SideOwner {
    BrokerHost,
    Target {
        name: String,
        daemon_instance_id: String,
    },
}

#[derive(Clone)]
pub enum SideHandle {
    Target {
        name: String,
        handle: TargetHandle,
        daemon_instance_id: String,
    },
    Local(LocalPortClient),
}

impl SideHandle {
    pub fn broker_host() -> anyhow::Result<Self> {
        Ok(Self::Local(LocalPortClient::global()?))
    }

    pub fn target(name: String, handle: TargetHandle, daemon_instance_id: String) -> Self {
        Self::Target {
            name,
            handle,
            daemon_instance_id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Target { name, .. } => name,
            Self::Local(_) => TARGET_NAME,
        }
    }

    pub(super) fn owner(&self) -> SideOwner {
        match self {
            Self::Target {
                name,
                daemon_instance_id,
                ..
            } => SideOwner::Target {
                name: name.clone(),
                daemon_instance_id: daemon_instance_id.clone(),
            },
            Self::Local(_) => SideOwner::BrokerHost,
        }
    }

    pub async fn port_tunnel(
        &self,
        max_queued_bytes: usize,
    ) -> Result<PortTunnel, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_tunnel(max_queued_bytes).await,
            Self::Local(client) => PortTunnel::local(client.state(), max_queued_bytes).await,
        }
    }
}
