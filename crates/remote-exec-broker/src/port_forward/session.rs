use std::sync::Arc;
use std::time::Duration;

use remote_exec_proto::public::ForwardPortProtocol as PublicForwardPortProtocol;
use tokio::sync::Mutex;

use super::side::SideHandle;
use super::tunnel::PortTunnel;

pub(super) const LISTEN_SESSION_STREAM_ID: u32 = 1;

pub(super) struct ListenSessionControl {
    pub(super) side: SideHandle,
    pub(super) forward_id: String,
    pub(super) session_id: String,
    pub(super) protocol: PublicForwardPortProtocol,
    pub(super) listener_stream_id: u32,
    pub(super) resume_timeout: Duration,
    pub(super) max_tunnel_queued_bytes: usize,
    state: Mutex<ListenSessionState>,
}

struct ListenSessionState {
    generation: u64,
    current_tunnel: Option<Arc<PortTunnel>>,
}

pub(super) struct ListenSessionSnapshot {
    pub(super) generation: u64,
    pub(super) current_tunnel: Option<Arc<PortTunnel>>,
}

pub(super) struct ListenSessionParams {
    pub(super) side: SideHandle,
    pub(super) forward_id: String,
    pub(super) session_id: String,
    pub(super) protocol: PublicForwardPortProtocol,
    pub(super) listener_stream_id: u32,
    pub(super) resume_timeout: Duration,
    pub(super) max_tunnel_queued_bytes: usize,
    pub(super) generation: u64,
    pub(super) tunnel: Arc<PortTunnel>,
}

impl ListenSessionControl {
    pub(super) fn new(params: ListenSessionParams) -> Self {
        Self {
            side: params.side,
            forward_id: params.forward_id,
            session_id: params.session_id,
            protocol: params.protocol,
            listener_stream_id: params.listener_stream_id,
            resume_timeout: params.resume_timeout,
            max_tunnel_queued_bytes: params.max_tunnel_queued_bytes,
            state: Mutex::new(ListenSessionState {
                generation: params.generation,
                current_tunnel: Some(params.tunnel),
            }),
        }
    }

    pub(super) async fn snapshot(&self) -> ListenSessionSnapshot {
        self.with_session_state(|state| ListenSessionSnapshot {
            generation: state.generation,
            current_tunnel: state.current_tunnel.clone(),
        })
        .await
    }

    pub(super) async fn current_tunnel(&self) -> Option<Arc<PortTunnel>> {
        self.snapshot().await.current_tunnel
    }

    pub(super) async fn advance_generation(&self) -> u64 {
        self.with_session_state(|state| {
            state.generation += 1;
            state.generation
        })
        .await
    }

    pub(super) async fn replace_current_tunnel(&self, generation: u64, tunnel: Arc<PortTunnel>) {
        self.with_session_state(|state| {
            state.generation = generation;
            state.current_tunnel = Some(tunnel);
        })
        .await;
    }

    async fn with_session_state<T>(
        &self,
        operation: impl FnOnce(&mut ListenSessionState) -> T,
    ) -> T {
        let mut state = self.state.lock().await;
        operation(&mut state)
    }

    #[cfg(test)]
    pub(super) fn new_for_test(
        side: SideHandle,
        forward_id: String,
        session_id: String,
        protocol: PublicForwardPortProtocol,
        resume_timeout: Duration,
        max_tunnel_queued_bytes: usize,
        tunnel: Option<Arc<PortTunnel>>,
    ) -> Self {
        Self {
            side,
            forward_id,
            session_id,
            protocol,
            listener_stream_id: LISTEN_SESSION_STREAM_ID,
            resume_timeout,
            max_tunnel_queued_bytes,
            state: Mutex::new(ListenSessionState {
                generation: super::epoch::INITIAL_FORWARD_GENERATION,
                current_tunnel: tunnel,
            }),
        }
    }
}
