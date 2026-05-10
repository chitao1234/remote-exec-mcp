use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use remote_exec_proto::port_forward::udp_connector_endpoint;
use remote_exec_proto::port_tunnel::{Frame, FrameType};
use tokio::sync::Mutex;

use super::apply_forward_drop_report;
use super::events::{ForwardLoopControl, ForwardSideEvent, TunnelRole, classify_transport_failure};
use super::generation::StreamIdAllocator;
use super::supervisor::{ForwardRuntime, reconnect_connect_tunnel, recover_listen_side_tunnels};
use super::tunnel::{
    EndpointMeta, PortTunnel, UdpDatagramMeta, classify_recoverable_tunnel_event,
    decode_tunnel_error_frame, decode_tunnel_meta, encode_tunnel_meta,
    format_terminal_tunnel_error, is_backpressure_error, is_recoverable_pressure_tunnel_error,
};
use super::{UDP_CONNECTOR_IDLE_SWEEP_INTERVAL, UDP_CONNECTOR_IDLE_TIMEOUT};

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
                let Some(recovered) = recover_listen_side_tunnels(&runtime).await? else {
                    return Ok(());
                };
                listen_tunnel = recovered.listen_tunnel;
                connect_tunnel = recovered.connect_tunnel;
            }
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
                runtime
                    .store
                    .update_entry(&runtime.forward_id, |entry| {
                        entry.dropped_udp_datagrams += 1;
                    })
                    .await;
                let Some(reconnected_tunnel) = reconnect_connect_tunnel(&runtime).await? else {
                    return Ok(());
                };
                connect_tunnel = reconnected_tunnel;
            }
        }
    }
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
    let mut connector_stream_ids = StreamIdAllocator::new_odd_from(3);
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
                        let connector_stream_id = match udp_connector_stream_id(
                            &connect_tunnel,
                            &connector_by_peer,
                            &peer_by_connector,
                            &mut connector_stream_ids,
                            &connector_bind_endpoint,
                            datagram.peer.clone(),
                            runtime.max_udp_peers_per_forward,
                        ).await {
                            Ok(stream_id) => stream_id,
                            Err(UdpConnectorError::LimitExceeded) => {
                                runtime
                                    .store
                                    .update_entry(&runtime.forward_id, |entry| {
                                        entry.dropped_udp_datagrams += 1;
                                    })
                                    .await;
                                continue;
                            }
                            Err(UdpConnectorError::Transport(err)) => {
                                runtime
                                    .store
                                    .update_entry(&runtime.forward_id, |entry| {
                                        entry.dropped_udp_datagrams += 1;
                                    })
                                    .await;
                                return classify_transport_failure(
                                    err,
                                    "opening udp connector stream",
                                    TunnelRole::Connect,
                                );
                            }
                        };
                        if let Err(err) = connect_tunnel.send(Frame {
                            frame_type: FrameType::UdpDatagram,
                            flags: 0,
                            stream_id: connector_stream_id,
                            meta: encode_tunnel_meta(&UdpDatagramMeta {
                                peer: runtime.connect_endpoint.clone(),
                            })?,
                            data: frame.data,
                        }).await {
                            runtime
                                .store
                                .update_entry(&runtime.forward_id, |entry| {
                                    entry.dropped_udp_datagrams += 1;
                                })
                                .await;
                            if is_backpressure_error(&err) {
                                continue;
                            }
                            return classify_transport_failure(
                                err,
                                "relaying udp datagram to connect tunnel",
                                TunnelRole::Connect,
                            );
                        }
                    }
                    FrameType::Close => return Ok(ForwardLoopControl::Cancelled),
                    FrameType::Error if frame.stream_id == runtime.listen_session.listener_stream_id => {
                        let meta = decode_tunnel_error_frame(&frame);
                        if is_recoverable_pressure_tunnel_error(&meta) {
                            runtime
                                .store
                                .update_entry(&runtime.forward_id, |entry| {
                                    entry.dropped_udp_datagrams += 1;
                                })
                                .await;
                            continue;
                        }
                        return Err(format_terminal_tunnel_error(&meta))
                            .context("listen-side udp tunnel error");
                    }
                    FrameType::ForwardDrop => {
                        apply_forward_drop_report(
                            &runtime.store,
                            &runtime.forward_id,
                            &frame,
                        )
                        .await?;
                    }
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
                    FrameType::Error => {
                        runtime
                            .store
                            .update_entry(&runtime.forward_id, |entry| {
                                entry.dropped_udp_datagrams += 1;
                            })
                            .await;
                        remove_udp_connector(
                            &connector_by_peer,
                            &peer_by_connector,
                            frame.stream_id,
                        )
                        .await;
                    }
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
    connector_stream_ids: &mut StreamIdAllocator,
    connector_bind_endpoint: &str,
    peer: String,
    max_udp_peers: usize,
) -> Result<u32, UdpConnectorError> {
    if let Some(connector) = connector_by_peer.lock().await.get_mut(&peer) {
        connector.last_used = Instant::now();
        return Ok(connector.stream_id);
    }

    if connector_by_peer.lock().await.len() >= max_udp_peers {
        return Err(UdpConnectorError::LimitExceeded);
    }

    let stream_id = connector_stream_ids.next().ok_or_else(|| {
        debug_assert!(connector_stream_ids.needs_generation_rotation());
        UdpConnectorError::Transport(anyhow::anyhow!(
            "port tunnel stream id generation exhausted"
        ))
    })?;
    connect_tunnel
        .send(Frame {
            frame_type: FrameType::UdpBind,
            flags: 0,
            stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: connector_bind_endpoint.to_string(),
            })
            .map_err(UdpConnectorError::Transport)?,
            data: Vec::new(),
        })
        .await
        .map_err(UdpConnectorError::Transport)?;
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

enum UdpConnectorError {
    LimitExceeded,
    Transport(anyhow::Error),
}

async fn remove_udp_connector(
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
    stream_id: u32,
) {
    if let Some(peer) = peer_by_connector.lock().await.remove(&stream_id) {
        connector_by_peer.lock().await.remove(&peer);
    }
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
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::task::{Context as TaskContext, Poll, Waker};
    use std::time::{Duration, Instant};

    use remote_exec_proto::port_tunnel::{
        ForwardDropKind, ForwardDropMeta, HEADER_LEN, write_frame,
    };
    use remote_exec_proto::public::{
        ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::Mutex;
    use tokio::sync::Mutex as TokioMutex;
    use tokio_util::sync::CancellationToken;

    use super::super::side::SideHandle;
    use super::super::supervisor::{ForwardRuntime, ListenSessionControl};
    use super::super::tunnel::PortTunnel;
    use super::*;

    #[derive(Clone, Default)]
    struct ScriptedTunnelIo {
        state: Arc<StdMutex<ScriptedTunnelState>>,
    }

    #[derive(Default)]
    struct ScriptedTunnelState {
        read_bytes: VecDeque<u8>,
        written_bytes: Vec<u8>,
        fail_writes: bool,
        read_waker: Option<Waker>,
    }

    impl ScriptedTunnelIo {
        fn fail_writes(&self) {
            self.state.lock().unwrap().fail_writes = true;
        }

        async fn wait_for_written_frame(&self, frame_type: FrameType, stream_id: u32) {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if self.pop_matching_written_frame(frame_type, stream_id) {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("expected tunnel frame should be written");
        }

        fn pop_matching_written_frame(&self, frame_type: FrameType, stream_id: u32) -> bool {
            let mut state = self.state.lock().unwrap();
            if state.written_bytes.len() < HEADER_LEN {
                return false;
            }
            let meta_len = u32::from_be_bytes(
                state.written_bytes[8..12]
                    .try_into()
                    .expect("header slice length"),
            ) as usize;
            let data_len = u32::from_be_bytes(
                state.written_bytes[12..16]
                    .try_into()
                    .expect("header slice length"),
            ) as usize;
            let frame_len = HEADER_LEN + meta_len + data_len;
            if state.written_bytes.len() < frame_len {
                return false;
            }
            let written_frame_type = state.written_bytes[0];
            let written_stream_id = u32::from_be_bytes(
                state.written_bytes[4..8]
                    .try_into()
                    .expect("header slice length"),
            );
            assert_eq!(written_frame_type, frame_type as u8);
            assert_eq!(written_stream_id, stream_id);
            state.written_bytes.drain(..frame_len);
            true
        }
    }

    impl AsyncRead for ScriptedTunnelIo {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let mut state = self.state.lock().unwrap();
            if state.read_bytes.is_empty() {
                state.read_waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let read = buf.remaining().min(state.read_bytes.len());
            let bytes: Vec<u8> = state.read_bytes.drain(..read).collect();
            buf.put_slice(&bytes);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for ScriptedTunnelIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut state = self.state.lock().unwrap();
            if state.fail_writes {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "forced closed writer",
                )));
            }
            state.written_bytes.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn udp_connector_limit_refuses_new_peer_without_evicting_existing_peer() {
        let tunnel = SideHandle::local().unwrap()
            .port_tunnel(PortTunnel::DEFAULT_MAX_QUEUED_BYTES)
            .await
            .unwrap();
        let connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let peer_by_connector: Arc<Mutex<HashMap<u32, String>>> =
            Arc::new(Mutex::new(HashMap::new()));

        const MAX_UDP_PEERS: usize = 3;
        for idx in 0..MAX_UDP_PEERS {
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

        let mut connector_stream_ids = StreamIdAllocator::new_odd_from(9);
        let result = udp_connector_stream_id(
            &Arc::new(tunnel),
            &connector_by_peer,
            &peer_by_connector,
            &mut connector_stream_ids,
            "127.0.0.1:0",
            "127.0.0.1:65535".to_string(),
            MAX_UDP_PEERS,
        )
        .await;
        assert!(matches!(result, Err(UdpConnectorError::LimitExceeded)));

        let connectors = connector_by_peer.lock().await;
        assert_eq!(connectors.len(), MAX_UDP_PEERS);
        assert!(!connectors.contains_key("127.0.0.1:65535"));
        drop(connectors);

        let peers = peer_by_connector.lock().await;
        assert_eq!(peers.len(), MAX_UDP_PEERS);
    }

    #[tokio::test]
    async fn udp_bind_send_failure_recovers_connect_tunnel() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        connect_io.fail_writes();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
        wait_until_send_fails(&connect_tunnel).await;

        let runtime = udp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "peer": "127.0.0.1:11001"
                }))
                .unwrap(),
                data: b"first".to_vec(),
            },
        )
        .await
        .unwrap();

        let control = tokio::time::timeout(
            Duration::from_secs(1),
            run_udp_forward_epoch(&runtime, listen_tunnel, connect_tunnel),
        )
        .await
        .expect("udp epoch should finish after retryable bind send failure")
        .expect("retryable udp bind send failure should recover connect tunnel");
        assert!(matches!(
            control,
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)
        ));
    }

    #[tokio::test]
    async fn udp_datagram_send_failure_recovers_connect_tunnel() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
        let runtime = udp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());

        let epoch_runtime = runtime.clone();
        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_udp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "peer": "127.0.0.1:11002"
                }))
                .unwrap(),
                data: b"first".to_vec(),
            },
        )
        .await
        .unwrap();
        connect_io
            .wait_for_written_frame(FrameType::UdpBind, 3)
            .await;
        connect_io
            .wait_for_written_frame(FrameType::UdpDatagram, 3)
            .await;

        connect_io.fail_writes();
        wait_until_send_fails(&connect_tunnel).await;
        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "peer": "127.0.0.1:11002"
                }))
                .unwrap(),
                data: b"after-loss".to_vec(),
            },
        )
        .await
        .unwrap();

        let control = tokio::time::timeout(Duration::from_secs(1), epoch)
            .await
            .expect("udp epoch should finish after retryable datagram send failure")
            .unwrap()
            .expect("retryable udp datagram send failure should recover connect tunnel");
        assert!(matches!(
            control,
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)
        ));
    }

    #[tokio::test]
    async fn udp_listener_error_fails_forward() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
        let runtime = udp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::Error,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "code": "port_read_failed",
                    "message": "udp read loop stopped",
                    "fatal": false
                }))
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            run_udp_forward_epoch(&runtime, listen_tunnel, connect_tunnel),
        )
        .await
        .expect("udp epoch should finish after listener error");
        let err = match result {
            Ok(_) => panic!("listener stream error should fail the forward"),
            Err(err) => err,
        };
        assert_eq!(
            format!("{err:#}"),
            "listen-side udp tunnel error: port_read_failed: udp read loop stopped"
        );
    }

    #[tokio::test]
    async fn udp_listener_pressure_error_counts_drop_without_failing_forward() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
        let runtime = udp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
        runtime
            .store
            .insert(test_record(&runtime, "127.0.0.1:10000"))
            .await;
        let cancel = runtime.cancel.clone();

        let epoch_runtime = runtime.clone();
        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_udp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::Error,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "code": "port_tunnel_limit_exceeded",
                    "message": "port tunnel queued byte limit reached",
                    "fatal": false
                }))
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
                if entries[0].dropped_udp_datagrams == 1 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("listener pressure error should count a dropped udp datagram");

        cancel.cancel();
        let control = tokio::time::timeout(Duration::from_secs(1), epoch)
            .await
            .expect("udp epoch should stay alive after pressure error")
            .unwrap()
            .expect("pressure error should not fail the forward");
        assert!(matches!(control, ForwardLoopControl::Cancelled));
    }

    #[tokio::test]
    async fn udp_listener_forward_drop_counts_drop_without_failing_forward() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
        let runtime = udp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
        runtime
            .store
            .insert(test_record(&runtime, "127.0.0.1:10000"))
            .await;
        let cancel = runtime.cancel.clone();

        let epoch_runtime = runtime.clone();
        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_udp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::ForwardDrop,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&ForwardDropMeta {
                    kind: ForwardDropKind::UdpDatagram,
                    count: 3,
                    reason: "port_tunnel_limit_exceeded".to_string(),
                    message: Some("port tunnel queued byte limit reached".to_string()),
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
                if entries[0].dropped_udp_datagrams == 3 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("listener drop telemetry should count dropped udp datagrams");

        cancel.cancel();
        let control = tokio::time::timeout(Duration::from_secs(1), epoch)
            .await
            .expect("udp epoch should stay alive after drop telemetry")
            .unwrap()
            .expect("drop telemetry should not fail the forward");
        assert!(matches!(control, ForwardLoopControl::Cancelled));
    }

    fn udp_test_runtime(
        listen_tunnel: Arc<PortTunnel>,
        connect_tunnel: Arc<PortTunnel>,
    ) -> ForwardRuntime {
        let listen_session = Arc::new(ListenSessionControl {
            side: SideHandle::local().unwrap(),
            forward_id: "fwd_test".to_string(),
            session_id: "test-session".to_string(),
            protocol: PublicForwardPortProtocol::Udp,
            generation: 1,
            listener_stream_id: 1,
            resume_timeout: Duration::from_secs(30),
            max_tunnel_queued_bytes: PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            current_tunnel: TokioMutex::new(Some(listen_tunnel)),
            op_lock: TokioMutex::new(()),
        });
        ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local().unwrap(),
            connect_side: SideHandle::local().unwrap(),
            protocol: PublicForwardPortProtocol::Udp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            max_active_tcp_streams_per_forward: 256,
            max_pending_tcp_bytes_per_stream: 256 * 1024,
            max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
            max_udp_peers_per_forward: 256,
            max_tunnel_queued_bytes: PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            max_reconnecting_forwards: 16,
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel,
            cancel: CancellationToken::new(),
        }
    }

    fn filter_one(forward_id: &str) -> super::super::store::PortForwardFilter {
        super::super::store::PortForwardFilter {
            listen_side: None,
            connect_side: None,
            forward_ids: vec![forward_id.to_string()],
        }
    }

    fn test_record(
        runtime: &ForwardRuntime,
        listen_endpoint: &str,
    ) -> super::super::store::PortForwardRecord {
        super::super::store::PortForwardRecord {
            entry: ForwardPortEntry::new_open(
                runtime.forward_id.clone(),
                runtime.listen_side.name().to_string(),
                listen_endpoint.to_string(),
                runtime.connect_side.name().to_string(),
                runtime.connect_endpoint.clone(),
                runtime.protocol,
                ForwardPortLimitSummary {
                    max_active_tcp_streams: runtime.max_active_tcp_streams_per_forward,
                    max_udp_peers: runtime.max_udp_peers_per_forward as u64,
                    max_pending_tcp_bytes_per_stream: runtime.max_pending_tcp_bytes_per_stream
                        as u64,
                    max_pending_tcp_bytes_per_forward: runtime.max_pending_tcp_bytes_per_forward
                        as u64,
                    max_tunnel_queued_bytes: runtime.max_tunnel_queued_bytes as u64,
                    max_reconnecting_forwards: runtime.max_reconnecting_forwards,
                },
            ),
            listen_session: runtime.listen_session.clone(),
            cancel: runtime.cancel.clone(),
            task_done: Arc::new(TokioMutex::new(None)),
        }
    }

    async fn wait_until_send_fails(tunnel: &PortTunnel) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let result = tunnel
                    .send(Frame {
                        frame_type: FrameType::Close,
                        flags: 0,
                        stream_id: 99,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                if result.is_err() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connect tunnel writer should close after forced write failure");
    }
}
