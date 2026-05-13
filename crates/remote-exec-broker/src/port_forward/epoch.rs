use std::sync::Arc;

use super::tunnel::PortTunnel;

pub(super) const INITIAL_FORWARD_GENERATION: u64 = 1;

#[derive(Clone)]
pub(super) struct ForwardEpoch {
    generation: u64,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
}

impl ForwardEpoch {
    pub(super) fn new(
        generation: u64,
        listen_tunnel: Arc<PortTunnel>,
        connect_tunnel: Arc<PortTunnel>,
    ) -> Self {
        Self {
            generation,
            listen_tunnel,
            connect_tunnel,
        }
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn listen_tunnel(&self) -> &Arc<PortTunnel> {
        &self.listen_tunnel
    }

    pub(super) fn connect_tunnel(&self) -> &Arc<PortTunnel> {
        &self.connect_tunnel
    }
}
