mod broker_tls;
mod client;
mod config;
mod daemon_client;
mod local;
mod logging;
mod mcp_server;
mod port_forward;
mod request_context;
mod session_store;
mod startup;
mod state;
mod target;
mod tools;

pub use client::{Connection, RemoteExecClient, ToolResponse};
pub use config::{BrokerConfig, ValidatedBrokerConfig};
pub use logging::init_logging;
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
