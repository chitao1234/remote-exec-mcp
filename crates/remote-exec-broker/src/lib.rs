pub(crate) mod broker_tls;
pub mod client;
pub mod config;
pub mod daemon_client;
pub mod local_backend;
pub(crate) mod local_port_backend;
pub mod local_transfer;
pub mod logging;
pub mod mcp_server;
pub mod port_forward;
pub mod session_store;
mod startup;
mod state;
mod target;
pub mod tools;

pub use startup::{build_state, run};
pub use state::BrokerState;
pub use target::{CachedDaemonInfo, TargetHandle};

pub fn install_crypto_provider() -> anyhow::Result<()> {
    broker_tls::install_crypto_provider()
}

#[cfg(test)]
#[test]
fn stream_id_allocator_rotates_before_wrap() {
    let mut allocator = port_forward::generation::StreamIdAllocator::new_odd();
    allocator.set_next_for_test(u32::MAX - 2);
    assert_eq!(allocator.next().unwrap(), u32::MAX - 2);
    assert!(allocator.next().is_none());
    assert!(allocator.needs_generation_rotation());
}
