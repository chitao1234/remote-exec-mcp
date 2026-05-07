use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use remote_exec_proto::port_forward::udp_connector_endpoint;
use remote_exec_proto::port_tunnel::{Frame, FrameType};
use tokio::sync::Mutex;

use super::events::{ForwardLoopControl, ForwardSideEvent, TunnelRole, classify_transport_failure};
use super::supervisor::{ForwardRuntime, open_connect_tunnel, reconnect_listen_tunnel};
use super::tunnel::{
    EndpointMeta, PortTunnel, UdpDatagramMeta, classify_recoverable_tunnel_event,
    decode_tunnel_meta, encode_tunnel_meta, format_terminal_tunnel_error,
};
use super::{
    MAX_UDP_CONNECTORS_PER_FORWARD, UDP_CONNECTOR_IDLE_SWEEP_INTERVAL, UDP_CONNECTOR_IDLE_TIMEOUT,
};

struct UdpPeerConnector {
    stream_id: u32,
    last_used: Instant,
}

pub(super) async fn run_udp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let mut listen_tunnel = runtime
        .listen_session
        .current_tunnel()
        .await
        .context("missing listen-side port tunnel")?;
    let mut connect_tunnel = runtime.initial_connect_tunnel.clone();

    loop {
        match run_udp_forward_epoch(&runtime, listen_tunnel.clone(), connect_tunnel.clone()).await?
        {
            ForwardLoopControl::Cancelled => return Ok(()),
            ForwardLoopControl::RecoverTunnel(TunnelRole::Listen) => {
                let Some(resumed_tunnel) =
                    reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone())
                        .await?
                else {
                    return Ok(());
                };
                listen_tunnel = resumed_tunnel;
                connect_tunnel = open_connect_tunnel(&runtime.connect_side)
                    .await
                    .with_context(|| {
                        format!(
                            "reopening port tunnel to `{}` after listen-side reconnect",
                            runtime.connect_side.name()
                        )
                    })?;
            }
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
                connect_tunnel =
                    reopen_connect_tunnel(&runtime, "after connect-side reconnect").await?;
            }
        }
    }
}

async fn reopen_connect_tunnel(
    runtime: &ForwardRuntime,
    reason: &str,
) -> anyhow::Result<Arc<PortTunnel>> {
    open_connect_tunnel(&runtime.connect_side)
        .await
        .with_context(|| {
            format!(
                "reopening port tunnel to `{}` {reason}",
                runtime.connect_side.name()
            )
        })
}

async fn run_udp_forward_epoch(
    runtime: &ForwardRuntime,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> anyhow::Result<ForwardLoopControl> {
    let connector_bind_endpoint = udp_connector_endpoint(&runtime.connect_endpoint)?.to_string();
    let connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let peer_by_connector: Arc<Mutex<HashMap<u32, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut next_connector_stream_id = 3;
    let mut sweep = tokio::time::interval(UDP_CONNECTOR_IDLE_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            _ = sweep.tick() => {
                sweep_idle_udp_connectors(
                    &connect_tunnel,
                    &connector_by_peer,
                    &peer_by_connector,
                ).await;
            }
            frame = listen_tunnel.recv() => {
                let frame = match classify_recoverable_tunnel_event(frame) {
                    ForwardSideEvent::Frame(frame) => frame,
                    ForwardSideEvent::RetryableTransportLoss => {
                        return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Listen));
                    }
                    ForwardSideEvent::TerminalTransportError(err) => {
                        return Err(err).context("reading udp listen tunnel");
                    }
                    ForwardSideEvent::TerminalTunnelError(meta) => {
                        return Err(format_terminal_tunnel_error(&meta))
                            .context("listen-side udp tunnel error");
                    }
                };
                match frame.frame_type {
                    FrameType::UdpDatagram => {
                        let datagram: UdpDatagramMeta = decode_tunnel_meta(&frame)?;
                        let connector_stream_id = udp_connector_stream_id(
                            &connect_tunnel,
                            &connector_by_peer,
                            &peer_by_connector,
                            &mut next_connector_stream_id,
                            &connector_bind_endpoint,
                            datagram.peer.clone(),
                        ).await?;
                        connect_tunnel.send(Frame {
                            frame_type: FrameType::UdpDatagram,
                            flags: 0,
                            stream_id: connector_stream_id,
                            meta: encode_tunnel_meta(&UdpDatagramMeta {
                                peer: runtime.connect_endpoint.clone(),
                            })?,
                            data: frame.data,
                        }).await.context("relaying udp datagram to connect tunnel")?;
                    }
                    FrameType::Close => return Ok(ForwardLoopControl::Cancelled),
                    _ => {}
                }
            }
            frame = connect_tunnel.recv() => {
                let frame = match classify_recoverable_tunnel_event(frame) {
                    ForwardSideEvent::Frame(frame) => frame,
                    ForwardSideEvent::RetryableTransportLoss => {
                        return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect));
                    }
                    ForwardSideEvent::TerminalTransportError(err) => {
                        return Err(err).context("reading udp connect tunnel");
                    }
                    ForwardSideEvent::TerminalTunnelError(meta) => {
                        return Err(format_terminal_tunnel_error(&meta))
                            .context("connect-side udp tunnel error");
                    }
                };
                match frame.frame_type {
                    FrameType::UdpBindOk => {}
                    FrameType::UdpDatagram => {
                        let Some(peer) = peer_by_connector.lock().await.get(&frame.stream_id).cloned() else {
                            continue;
                        };
                        if let Some(connector) = connector_by_peer.lock().await.get_mut(&peer) {
                            connector.last_used = Instant::now();
                        }
                        if let Err(err) = listen_tunnel.send(Frame {
                            frame_type: FrameType::UdpDatagram,
                            flags: 0,
                            stream_id: 1,
                            meta: encode_tunnel_meta(&UdpDatagramMeta { peer })?,
                            data: frame.data,
                        }).await {
                            return classify_transport_failure(
                                err,
                                "relaying udp datagram to listen tunnel",
                                TunnelRole::Listen,
                            );
                        }
                    }
                    FrameType::Close => {
                        if let Some(peer) = peer_by_connector.lock().await.remove(&frame.stream_id) {
                            connector_by_peer.lock().await.remove(&peer);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn udp_connector_stream_id(
    connect_tunnel: &Arc<PortTunnel>,
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
    next_connector_stream_id: &mut u32,
    connector_bind_endpoint: &str,
    peer: String,
) -> anyhow::Result<u32> {
    if let Some(connector) = connector_by_peer.lock().await.get_mut(&peer) {
        connector.last_used = Instant::now();
        return Ok(connector.stream_id);
    }

    evict_udp_connector_if_needed(connect_tunnel, connector_by_peer, peer_by_connector).await;

    let stream_id = *next_connector_stream_id;
    *next_connector_stream_id = next_connector_stream_id.checked_add(2).unwrap_or(1);
    connect_tunnel
        .send(Frame {
            frame_type: FrameType::UdpBind,
            flags: 0,
            stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: connector_bind_endpoint.to_string(),
            })?,
            data: Vec::new(),
        })
        .await
        .context("opening udp connector stream")?;
    connector_by_peer.lock().await.insert(
        peer.clone(),
        UdpPeerConnector {
            stream_id,
            last_used: Instant::now(),
        },
    );
    peer_by_connector.lock().await.insert(stream_id, peer);
    Ok(stream_id)
}

async fn evict_udp_connector_if_needed(
    connect_tunnel: &Arc<PortTunnel>,
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
) {
    let candidate = {
        let connectors = connector_by_peer.lock().await;
        if connectors.len() < MAX_UDP_CONNECTORS_PER_FORWARD {
            return;
        }
        connectors
            .iter()
            .min_by_key(|(_, connector)| connector.last_used)
            .map(|(peer, connector)| (peer.clone(), connector.stream_id))
    };
    let Some((peer, stream_id)) = candidate else {
        return;
    };

    connector_by_peer.lock().await.remove(&peer);
    peer_by_connector.lock().await.remove(&stream_id);
    let _ = connect_tunnel.close_stream(stream_id).await;
}

async fn sweep_idle_udp_connectors(
    connect_tunnel: &Arc<PortTunnel>,
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
) {
    let now = Instant::now();
    let mut expired = Vec::new();
    {
        let connectors = connector_by_peer.lock().await;
        for (peer, connector) in connectors.iter() {
            if now.duration_since(connector.last_used) >= UDP_CONNECTOR_IDLE_TIMEOUT {
                expired.push((peer.clone(), connector.stream_id));
            }
        }
    }
    if expired.is_empty() {
        return;
    }
    let mut connectors = connector_by_peer.lock().await;
    let mut peers = peer_by_connector.lock().await;
    for (peer, stream_id) in expired {
        connectors.remove(&peer);
        peers.remove(&stream_id);
        let _ = connect_tunnel.close_stream(stream_id).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::sync::Mutex;

    use super::super::side::SideHandle;
    use super::*;

    #[tokio::test]
    async fn udp_connector_limit_evicts_stalest_peer() {
        let tunnel = SideHandle::local().port_tunnel().await.unwrap();
        let connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let peer_by_connector: Arc<Mutex<HashMap<u32, String>>> =
            Arc::new(Mutex::new(HashMap::new()));

        for idx in 0..MAX_UDP_CONNECTORS_PER_FORWARD {
            let stream_id = (idx as u32 * 2) + 3;
            let last_used = Instant::now() - Duration::from_secs((idx + 1) as u64);
            let peer = format!("127.0.0.1:{}", 10_000 + idx);
            connector_by_peer.lock().await.insert(
                peer.clone(),
                UdpPeerConnector {
                    stream_id,
                    last_used,
                },
            );
            peer_by_connector.lock().await.insert(stream_id, peer);
        }

        let stalest_peer = connector_by_peer
            .lock()
            .await
            .iter()
            .min_by_key(|(_, connector)| connector.last_used)
            .map(|(peer, connector)| (peer.clone(), connector.stream_id))
            .unwrap();

        let mut next_connector_stream_id = ((MAX_UDP_CONNECTORS_PER_FORWARD as u32) * 2) + 3;
        let created_stream_id = udp_connector_stream_id(
            &Arc::new(tunnel),
            &connector_by_peer,
            &peer_by_connector,
            &mut next_connector_stream_id,
            "127.0.0.1:0",
            "127.0.0.1:65535".to_string(),
        )
        .await
        .unwrap();

        let connectors = connector_by_peer.lock().await;
        assert_eq!(connectors.len(), MAX_UDP_CONNECTORS_PER_FORWARD);
        assert!(!connectors.contains_key(&stalest_peer.0));
        assert!(connectors.contains_key("127.0.0.1:65535"));
        assert_eq!(connectors["127.0.0.1:65535"].stream_id, created_stream_id);
        drop(connectors);

        let peers = peer_by_connector.lock().await;
        assert_eq!(peers.len(), MAX_UDP_CONNECTORS_PER_FORWARD);
        assert!(!peers.contains_key(&stalest_peer.1));
        assert_eq!(
            peers.get(&created_stream_id).map(String::as_str),
            Some("127.0.0.1:65535")
        );
    }
}
