use std::sync::Arc;
use std::sync::OnceLock;

use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectRequest, PortConnectResponse, PortConnectionCloseRequest,
    PortConnectionReadRequest, PortConnectionReadResponse, PortConnectionWriteRequest,
    PortLeaseRenewRequest, PortListenAcceptRequest, PortListenAcceptResponse,
    PortListenCloseRequest, PortListenRequest, PortListenResponse, PortUdpDatagramReadRequest,
    PortUdpDatagramReadResponse, PortUdpDatagramWriteRequest,
};

use crate::daemon_client::DaemonClientError;
use crate::local_backend::map_host_rpc_error;

#[derive(Clone)]
pub struct LocalPortClient {
    state: Arc<remote_exec_host::HostRuntimeState>,
}

impl Default for LocalPortClient {
    fn default() -> Self {
        Self::global()
    }
}

impl LocalPortClient {
    pub fn global() -> Self {
        static STATE: OnceLock<Arc<remote_exec_host::HostRuntimeState>> = OnceLock::new();
        let state = STATE
            .get_or_init(|| {
                let config = remote_exec_host::EmbeddedHostConfig {
                    target: "local".to_string(),
                    default_workdir: std::env::current_dir()
                        .unwrap_or_else(|_| std::env::temp_dir()),
                    windows_posix_root: None,
                    sandbox: None,
                    enable_transfer_compression: false,
                    allow_login_shell: false,
                    pty: remote_exec_host::PtyMode::None,
                    default_shell: None,
                    yield_time: remote_exec_host::YieldTimeConfig::default(),
                    experimental_apply_patch_target_encoding_autodetect: false,
                    process_environment: remote_exec_host::ProcessEnvironment::capture_current(),
                };
                Arc::new(
                    remote_exec_host::build_runtime_state(config.into_host_runtime_config())
                        .expect("construct local port runtime"),
                )
            })
            .clone();
        Self { state }
    }

    pub async fn port_listen(
        &self,
        req: &PortListenRequest,
    ) -> Result<PortListenResponse, DaemonClientError> {
        remote_exec_host::port_forward::listen_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_listen_accept(
        &self,
        req: &PortListenAcceptRequest,
    ) -> Result<PortListenAcceptResponse, DaemonClientError> {
        remote_exec_host::port_forward::listen_accept_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_listen_close(
        &self,
        req: &PortListenCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::listen_close_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_lease_renew(
        &self,
        req: &PortLeaseRenewRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::lease_renew_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connect(
        &self,
        req: &PortConnectRequest,
    ) -> Result<PortConnectResponse, DaemonClientError> {
        remote_exec_host::port_forward::connect_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connection_read(
        &self,
        req: &PortConnectionReadRequest,
    ) -> Result<PortConnectionReadResponse, DaemonClientError> {
        remote_exec_host::port_forward::connection_read_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connection_write(
        &self,
        req: &PortConnectionWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::connection_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_connection_close(
        &self,
        req: &PortConnectionCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::connection_close_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_udp_datagram_read(
        &self,
        req: &PortUdpDatagramReadRequest,
    ) -> Result<PortUdpDatagramReadResponse, DaemonClientError> {
        remote_exec_host::port_forward::udp_datagram_read_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }

    pub async fn port_udp_datagram_write(
        &self,
        req: &PortUdpDatagramWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        remote_exec_host::port_forward::udp_datagram_write_local(self.state.clone(), req.clone())
            .await
            .map_err(map_host_rpc_error)
    }
}
