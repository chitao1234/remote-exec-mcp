use crate::TargetHandle;
use crate::daemon_client::DaemonClientError;
use crate::local_port_backend::LocalPortClient;
use crate::state::LOCAL_TARGET_NAME;

use super::tunnel::PortTunnel;

#[derive(Clone)]
pub enum SideHandle {
    Target { name: String, handle: TargetHandle },
    Local(LocalPortClient),
}

impl SideHandle {
    pub fn local() -> anyhow::Result<Self> {
        Ok(Self::Local(LocalPortClient::global()?))
    }

    pub fn target(name: String, handle: TargetHandle) -> Self {
        Self::Target { name, handle }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Target { name, .. } => name,
            Self::Local(_) => LOCAL_TARGET_NAME,
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
