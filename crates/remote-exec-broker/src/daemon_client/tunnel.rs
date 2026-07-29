use remote_exec_proto::port_tunnel::{
    TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN,
};
use reqwest::header::{CONNECTION, CONTENT_LENGTH, UPGRADE};

use crate::port_tunnel_io::write_preface;

use super::{DaemonClient, DaemonClientError, RpcErrorDecodePolicy, decode_rpc_error};

// Upgrade covers the HTTP 101 handshake and reqwest's transition into raw I/O.
const PORT_TUNNEL_UPGRADE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
// Preface covers only the v4 tunnel greeting after the HTTP upgrade succeeds.
const PORT_TUNNEL_PREFACE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl DaemonClient {
    pub async fn port_tunnel(&self) -> Result<reqwest::Upgraded, DaemonClientError> {
        let started = std::time::Instant::now();
        let (request, connection_generation) = self.request_with_generation("/v1/port/tunnel");
        let request = request
            .header(CONNECTION, "Upgrade")
            .header(UPGRADE, UPGRADE_TOKEN)
            .header(TUNNEL_PROTOCOL_VERSION_HEADER, TUNNEL_PROTOCOL_VERSION)
            .header(CONTENT_LENGTH, "0");
        let response = match tokio::time::timeout(PORT_TUNNEL_UPGRADE_TIMEOUT, request.send()).await
        {
            Ok(Ok(response)) => response,
            Ok(Err(err)) if err.is_timeout() => {
                let recovery = self
                    .recover_connection_after_timeout(
                        "port tunnel upgrade",
                        Some(connection_generation),
                    )
                    .await;
                return Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "port tunnel upgrade timed out; {recovery}"
                )));
            }
            Ok(Err(err)) => {
                return Err(self.rpc_transport_error("/v1/port/tunnel", started, err));
            }
            Err(_) => {
                let recovery = self
                    .recover_connection_after_timeout(
                        "port tunnel upgrade",
                        Some(connection_generation),
                    )
                    .await;
                return Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "port tunnel upgrade timed out; {recovery}"
                )));
            }
        };
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            self.record_connection_success(connection_generation);
            return Err(decode_rpc_error(response, RpcErrorDecodePolicy::Lenient).await);
        }
        let mut upgraded =
            match tokio::time::timeout(PORT_TUNNEL_UPGRADE_TIMEOUT, response.upgrade()).await {
                Ok(upgraded) => upgraded,
                Err(_) => {
                    let recovery = self
                        .recover_connection_after_timeout(
                            "port tunnel upgrade",
                            Some(connection_generation),
                        )
                        .await;
                    return Err(DaemonClientError::Transport(anyhow::anyhow!(
                        "port tunnel upgrade timed out; {recovery}"
                    )));
                }
            }
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        match tokio::time::timeout(PORT_TUNNEL_PREFACE_TIMEOUT, write_preface(&mut upgraded)).await
        {
            Ok(result) => result,
            Err(_) => {
                let recovery = self
                    .recover_connection_after_timeout(
                        "port tunnel preface",
                        Some(connection_generation),
                    )
                    .await;
                return Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "port tunnel preface timed out; {recovery}"
                )));
            }
        }
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
        self.record_connection_success(connection_generation);
        Ok(upgraded)
    }
}
