use remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES;
use remote_exec_proto::port_tunnel::TunnelLimitSummary;
use remote_exec_proto::public::ForwardPortLimitSummary;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct BrokerPortForwardLimits {
    pub max_open_forwards_total: usize,
    pub max_forwards_per_side_pair: usize,
    pub max_active_tcp_streams_per_forward: u64,
    pub max_pending_tcp_bytes_per_stream: u64,
    pub max_pending_tcp_bytes_per_forward: u64,
    pub max_udp_peers_per_forward: u64,
    pub max_tunnel_queued_bytes: u64,
    pub max_reconnecting_forwards: usize,
}

impl Default for BrokerPortForwardLimits {
    fn default() -> Self {
        Self {
            max_open_forwards_total: 64,
            max_forwards_per_side_pair: 16,
            max_active_tcp_streams_per_forward: 256,
            max_pending_tcp_bytes_per_stream: 256 * 1024,
            max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
            max_udp_peers_per_forward: 256,
            max_tunnel_queued_bytes: DEFAULT_TUNNEL_QUEUE_BYTES,
            max_reconnecting_forwards: 16,
        }
    }
}

impl BrokerPortForwardLimits {
    pub fn public_summary(self) -> ForwardPortLimitSummary {
        ForwardPortLimitSummary {
            max_active_tcp_streams: self.max_active_tcp_streams_per_forward,
            max_udp_peers: self.max_udp_peers_per_forward,
            max_pending_tcp_bytes_per_stream: self.max_pending_tcp_bytes_per_stream,
            max_pending_tcp_bytes_per_forward: self.max_pending_tcp_bytes_per_forward,
            max_tunnel_queued_bytes: self.max_tunnel_queued_bytes,
            max_reconnecting_forwards: self.max_reconnecting_forwards,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_open_forwards_total > 0,
            "port_forward_limits.max_open_forwards_total must be greater than zero"
        );
        anyhow::ensure!(
            self.max_forwards_per_side_pair > 0,
            "port_forward_limits.max_forwards_per_side_pair must be greater than zero"
        );
        anyhow::ensure!(
            self.max_active_tcp_streams_per_forward > 0,
            "port_forward_limits.max_active_tcp_streams_per_forward must be greater than zero"
        );
        anyhow::ensure!(
            self.max_pending_tcp_bytes_per_stream > 0,
            "port_forward_limits.max_pending_tcp_bytes_per_stream must be greater than zero"
        );
        anyhow::ensure!(
            self.max_pending_tcp_bytes_per_forward >= self.max_pending_tcp_bytes_per_stream,
            "port_forward_limits.max_pending_tcp_bytes_per_forward must be at least max_pending_tcp_bytes_per_stream"
        );
        anyhow::ensure!(
            self.max_udp_peers_per_forward > 0,
            "port_forward_limits.max_udp_peers_per_forward must be greater than zero"
        );
        anyhow::ensure!(
            self.max_tunnel_queued_bytes > 0,
            "port_forward_limits.max_tunnel_queued_bytes must be greater than zero"
        );
        anyhow::ensure!(
            self.max_reconnecting_forwards > 0,
            "port_forward_limits.max_reconnecting_forwards must be greater than zero"
        );
        Ok(())
    }
}

pub(super) fn effective_forward_limits(
    broker: ForwardPortLimitSummary,
    listen: &TunnelLimitSummary,
    connect: &TunnelLimitSummary,
) -> ForwardPortLimitSummary {
    ForwardPortLimitSummary {
        max_active_tcp_streams: broker
            .max_active_tcp_streams
            .min(listen.max_active_tcp_streams)
            .min(connect.max_active_tcp_streams),
        max_udp_peers: broker
            .max_udp_peers
            .min(listen.max_udp_peers)
            .min(connect.max_udp_peers),
        max_pending_tcp_bytes_per_stream: broker.max_pending_tcp_bytes_per_stream,
        max_pending_tcp_bytes_per_forward: broker.max_pending_tcp_bytes_per_forward,
        max_tunnel_queued_bytes: broker
            .max_tunnel_queued_bytes
            .min(listen.max_queued_bytes)
            .min(connect.max_queued_bytes),
        max_reconnecting_forwards: broker.max_reconnecting_forwards,
    }
}
