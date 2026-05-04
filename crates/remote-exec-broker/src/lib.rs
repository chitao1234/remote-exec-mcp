pub(crate) mod broker_tls;
pub mod client;
pub mod config;
pub mod daemon_client;
pub mod local_backend;
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

pub fn install_crypto_provider() {
    broker_tls::install_crypto_provider();
}
