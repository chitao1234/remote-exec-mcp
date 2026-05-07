mod events;
mod side;
mod store;
mod supervisor;
mod tcp_bridge;
mod tunnel;
mod udp_bridge;

use std::time::Duration;

pub use side::SideHandle;
pub use store::{PortForwardFilter, PortForwardRecord, PortForwardStore, close_all, close_record};
pub use supervisor::{OpenedForward, open_forward};
pub(crate) use tunnel::PortTunnel;

const UDP_CONNECTOR_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const UDP_CONNECTOR_IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const LISTEN_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(50);
const LISTEN_RECONNECT_MAX_BACKOFF: Duration = Duration::from_millis(500);
const LISTEN_RECONNECT_SAFETY_MARGIN: Duration = Duration::from_millis(250);
const LISTEN_CLOSE_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const FORWARD_TASK_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PENDING_TCP_BYTES_PER_STREAM: usize = 256 * 1024;
const MAX_PENDING_TCP_BYTES_PER_FORWARD: usize = 2 * 1024 * 1024;
const MAX_UDP_CONNECTORS_PER_FORWARD: usize = 256;
