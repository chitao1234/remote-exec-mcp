use crate::TargetHandle;
use crate::daemon_client::DaemonClientError;
use crate::local_port_backend::LocalPortClient;

use super::tunnel::PortTunnel;

#[derive(Clone)]
pub enum SideHandle {
    Target { name: String, handle: TargetHandle },
    Local(LocalPortClient),
}

impl SideHandle {
    pub fn local() -> Self {
        Self::Local(LocalPortClient::global())
    }

    pub fn target(name: String, handle: TargetHandle) -> Self {
        Self::Target { name, handle }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Target { name, .. } => name,
            Self::Local(_) => "local",
        }
    }

    pub async fn port_tunnel(&self) -> Result<PortTunnel, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_tunnel().await,
            Self::Local(client) => PortTunnel::local(client.state()).await,
        }
    }
}
