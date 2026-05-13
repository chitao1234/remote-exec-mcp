mod open;
mod reconnect;

use std::sync::Arc;

use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    ForwardPortSideRole,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::epoch::ForwardEpoch;
use super::limits::BrokerPortForwardLimits;
use super::side::SideHandle;
use super::store::{PortForwardRecord, PortForwardStore};
use super::tcp_bridge::run_tcp_forward;
use super::tunnel::PortTunnel;
use super::udp_bridge::run_udp_forward;

pub(super) use super::session::ListenSessionControl;
pub(super) use super::session::{LISTEN_SESSION_STREAM_ID, ListenSessionParams};
pub use open::open_forward;
pub(super) use reconnect::{
    close_listen_session, handle_forward_loop_control, wait_for_forward_task_stop,
};

#[derive(Clone)]
pub(super) struct ForwardIdentity {
    forward_id: String,
    listen_side: SideHandle,
    connect_side: SideHandle,
    protocol: PublicForwardPortProtocol,
    connect_endpoint: String,
}

impl ForwardIdentity {
    pub(super) fn new(
        forward_id: String,
        listen_side: SideHandle,
        connect_side: SideHandle,
        protocol: PublicForwardPortProtocol,
        connect_endpoint: String,
    ) -> Self {
        Self {
            forward_id,
            listen_side,
            connect_side,
            protocol,
            connect_endpoint,
        }
    }

    pub(super) fn forward_id(&self) -> &str {
        &self.forward_id
    }

    pub(super) fn listen_side(&self) -> &SideHandle {
        &self.listen_side
    }

    pub(super) fn connect_side(&self) -> &SideHandle {
        &self.connect_side
    }

    pub(super) fn protocol(&self) -> PublicForwardPortProtocol {
        self.protocol
    }

    pub(super) fn connect_endpoint(&self) -> &str {
        &self.connect_endpoint
    }
}

#[derive(Clone)]
pub(super) struct ForwardRuntime {
    pub(super) identity: ForwardIdentity,
    pub(super) limits: ForwardLimits,
    pub(super) store: PortForwardStore,
    pub(super) listen_session: Arc<ListenSessionControl>,
    pub(super) initial_epoch: ForwardEpoch,
    pub(super) initial_connect_tunnel: Arc<PortTunnel>,
    pub(super) cancel: CancellationToken,
}

#[derive(Clone, Copy)]
pub(super) struct ForwardLimits {
    pub(super) max_active_tcp_streams: u64,
    pub(super) max_pending_tcp_bytes_per_stream: usize,
    pub(super) max_pending_tcp_bytes_per_forward: usize,
    pub(super) max_udp_peers: usize,
    pub(super) max_tunnel_queued_bytes: usize,
    pub(super) max_reconnecting_forwards: usize,
}

impl ForwardLimits {
    #[cfg(test)]
    pub(super) fn public_summary(self) -> ForwardPortLimitSummary {
        ForwardPortLimitSummary {
            max_active_tcp_streams: self.max_active_tcp_streams,
            max_udp_peers: self.max_udp_peers as u64,
            max_pending_tcp_bytes_per_stream: self.max_pending_tcp_bytes_per_stream as u64,
            max_pending_tcp_bytes_per_forward: self.max_pending_tcp_bytes_per_forward as u64,
            max_tunnel_queued_bytes: self.max_tunnel_queued_bytes as u64,
            max_reconnecting_forwards: self.max_reconnecting_forwards,
        }
    }
}

impl From<ForwardPortLimitSummary> for ForwardLimits {
    fn from(summary: ForwardPortLimitSummary) -> Self {
        Self {
            max_active_tcp_streams: summary.max_active_tcp_streams,
            max_pending_tcp_bytes_per_stream: summary.max_pending_tcp_bytes_per_stream as usize,
            max_pending_tcp_bytes_per_forward: summary.max_pending_tcp_bytes_per_forward as usize,
            max_udp_peers: summary.max_udp_peers as usize,
            max_tunnel_queued_bytes: summary.max_tunnel_queued_bytes as usize,
            max_reconnecting_forwards: summary.max_reconnecting_forwards,
        }
    }
}

impl Default for ForwardLimits {
    fn default() -> Self {
        BrokerPortForwardLimits::default().public_summary().into()
    }
}

impl ForwardRuntime {
    pub(super) fn new(
        identity: ForwardIdentity,
        limits: ForwardLimits,
        store: PortForwardStore,
        listen_session: Arc<ListenSessionControl>,
        initial_epoch: ForwardEpoch,
        cancel: CancellationToken,
    ) -> Self {
        let initial_connect_tunnel = initial_epoch.connect_tunnel().clone();
        Self {
            identity,
            limits,
            store,
            listen_session,
            initial_epoch,
            initial_connect_tunnel,
            cancel,
        }
    }

    pub(super) fn forward_id(&self) -> &str {
        self.identity.forward_id()
    }

    pub(super) fn listen_side(&self) -> &SideHandle {
        self.identity.listen_side()
    }

    pub(super) fn connect_side(&self) -> &SideHandle {
        self.identity.connect_side()
    }

    pub(super) fn protocol(&self) -> PublicForwardPortProtocol {
        self.identity.protocol()
    }

    pub(super) fn connect_endpoint(&self) -> &str {
        self.identity.connect_endpoint()
    }

    pub(super) fn initial_epoch(&self) -> &ForwardEpoch {
        &self.initial_epoch
    }

    pub(super) async fn record_dropped_datagram(&self) {
        self.store
            .update_entry(self.forward_id(), |entry| {
                entry.dropped_udp_datagrams += 1;
            })
            .await;
    }

    pub(super) async fn record_dropped_stream(&self) {
        self.store
            .update_entry(self.forward_id(), |entry| {
                entry.dropped_tcp_streams += 1;
            })
            .await;
    }

    pub(super) async fn record_dropped_streams_and_release_active(&self, count: u64) {
        if count == 0 {
            return;
        }
        self.store
            .update_entry(self.forward_id(), |entry| {
                entry.dropped_tcp_streams += count;
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(count);
            })
            .await;
    }

    pub(super) async fn release_active_stream(&self) {
        self.store
            .update_entry(self.forward_id(), |entry| {
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(1);
            })
            .await;
    }

    pub(super) async fn record_dropped_active_stream(&self) {
        self.store
            .update_entry(self.forward_id(), |entry| {
                entry.dropped_tcp_streams += 1;
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(1);
            })
            .await;
    }

    pub(super) async fn mark_reconnecting(
        &self,
        side: ForwardPortSideRole,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.store
            .mark_reconnecting(
                self.forward_id(),
                side,
                reason.to_string(),
                self.limits.max_reconnecting_forwards,
            )
            .await
    }

    pub(super) async fn mark_active(&self, side: ForwardPortSideRole) {
        self.store.mark_ready(self.forward_id(), side).await;
    }
}

pub struct OpenedForward {
    pub record: PortForwardRecord,
    runtime: ForwardRuntime,
}

impl OpenedForward {
    pub fn entry(&self) -> &ForwardPortEntry {
        &self.record.entry
    }

    pub async fn register_and_start(self, store: super::store::PortForwardStore) {
        let runtime = self.runtime;
        let task = spawn_forward(runtime, store.clone());
        self.record.set_task(task).await;
        store.insert(self.record).await;
    }
}

fn spawn_forward(runtime: ForwardRuntime, store: super::store::PortForwardStore) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result = match runtime.protocol() {
            PublicForwardPortProtocol::Tcp => run_tcp_forward(runtime.clone()).await,
            PublicForwardPortProtocol::Udp => run_udp_forward(runtime.clone()).await,
        };
        if let Err(err) = result {
            let error_text = format!("{err:#}");
            runtime.cancel.cancel();
            store
                .mark_failed(runtime.forward_id(), error_text.clone())
                .await;
            tracing::warn!(
                forward_id = %runtime.forward_id(),
                listen_side = %runtime.listen_side().name(),
                connect_side = %runtime.connect_side().name(),
                error = %error_text,
                "port forward task stopped"
            );
        }
    })
}
