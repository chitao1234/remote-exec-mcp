mod events;
pub(crate) mod generation;
mod limits;
mod side;
mod store;
mod supervisor;
mod tcp_bridge;
mod tunnel;
mod udp_bridge;
mod udp_connectors;

use std::time::Duration;

use remote_exec_proto::port_tunnel::{ForwardDropKind, ForwardDropMeta, Frame};

pub use limits::BrokerPortForwardLimits;
pub use side::SideHandle;
pub use store::{PortForwardFilter, PortForwardRecord, PortForwardStore, close_all, close_record};
pub use supervisor::{OpenedForward, open_forward};
pub(crate) use tunnel::PortTunnel;

const UDP_CONNECTOR_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const UDP_CONNECTOR_IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const LISTEN_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(50);
const LISTEN_RECONNECT_MAX_BACKOFF: Duration = Duration::from_millis(500);
const LISTEN_RECONNECT_SAFETY_MARGIN: Duration = Duration::from_millis(250);
const PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_RECONNECT_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);
const LISTEN_CLOSE_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const PORT_FORWARD_OPEN_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const PORT_FORWARD_TUNNEL_READY_TIMEOUT: Duration = Duration::from_secs(5);
const FORWARD_TASK_STOP_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(not(test))]
const PORT_TUNNEL_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
#[cfg(test)]
const PORT_TUNNEL_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(not(test))]
const PORT_TUNNEL_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const PORT_TUNNEL_HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(250);

pub(super) fn port_tunnel_heartbeat_interval() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(duration) =
        test_duration_override("REMOTE_EXEC_TEST_PORT_TUNNEL_HEARTBEAT_INTERVAL_MS")
    {
        return duration;
    }
    PORT_TUNNEL_HEARTBEAT_INTERVAL
}

pub(super) fn port_tunnel_heartbeat_timeout() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(duration) =
        test_duration_override("REMOTE_EXEC_TEST_PORT_TUNNEL_HEARTBEAT_TIMEOUT_MS")
    {
        return duration;
    }
    PORT_TUNNEL_HEARTBEAT_TIMEOUT
}

#[cfg(debug_assertions)]
fn test_duration_override(name: &str) -> Option<Duration> {
    let millis = std::env::var(name).ok()?.parse::<u64>().ok()?;
    Some(Duration::from_millis(millis))
}

async fn apply_forward_drop_report(
    store: &PortForwardStore,
    forward_id: &str,
    frame: &Frame,
) -> anyhow::Result<()> {
    let meta: ForwardDropMeta = serde_json::from_slice(&frame.meta)?;
    let count = meta.count.max(1);
    store
        .update_entry(forward_id, |entry| match meta.kind {
            ForwardDropKind::TcpStream => {
                entry.dropped_tcp_streams = entry.dropped_tcp_streams.saturating_add(count);
            }
            ForwardDropKind::UdpDatagram => {
                entry.dropped_udp_datagrams = entry.dropped_udp_datagrams.saturating_add(count);
            }
        })
        .await;
    Ok(())
}
