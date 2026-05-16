use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use remote_exec_proto::port_forward::udp_connector_endpoint;
use remote_exec_proto::port_tunnel::{EndpointMeta, Frame, FrameType, UdpDatagramMeta};

use super::apply_forward_drop_report;
use super::events::{
    ForwardLoopControl, TunnelFrameOutcome, TunnelRole, classify_transport_failure,
    recoverable_tunnel_frame,
};
use super::generation::StreamIdAllocator;
use super::supervisor::{ForwardRuntime, handle_forward_loop_control};
use super::tunnel::{
    PortTunnel, decode_tunnel_error_frame, decode_tunnel_meta, encode_tunnel_meta,
    format_terminal_tunnel_error, is_backpressure_error, is_recoverable_pressure_tunnel_error,
};
use super::udp_connectors::{UdpConnectorMap, UdpPeerConnector};
use super::{UDP_CONNECTOR_IDLE_SWEEP_INTERVAL, UDP_CONNECTOR_IDLE_TIMEOUT};

pub(super) async fn run_udp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let mut epoch = runtime.initial_epoch().clone();

    loop {
        let control = run_udp_forward_epoch(&runtime, &epoch).await?;
        if !handle_forward_loop_control(&runtime, control, &mut epoch, || {
            runtime.record_dropped_datagram()
        })
        .await?
        {
            return Ok(());
        }
    }
}

async fn run_udp_forward_epoch(
    runtime: &ForwardRuntime,
    epoch: &super::epoch::ForwardEpoch,
) -> anyhow::Result<ForwardLoopControl> {
    let listen_tunnel = epoch.listen_tunnel().clone();
    let connect_tunnel = epoch.connect_tunnel().clone();
    let connector_bind_endpoint = udp_connector_endpoint(runtime.connect_endpoint())?.to_string();
    let connectors = UdpConnectorMap::default();
    let mut connector_stream_ids = StreamIdAllocator::new_odd_from(3);
    let mut sweep = tokio::time::interval(UDP_CONNECTOR_IDLE_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            _ = sweep.tick() => {
                sweep_idle_udp_connectors(
                    &connect_tunnel,
                    &connectors,
                ).await;
            }
            frame = listen_tunnel.recv() => {
                if let Some(control) = handle_listen_udp_tunnel_event(
                    runtime,
                    &connect_tunnel,
                    &connectors,
                    &mut connector_stream_ids,
                    &connector_bind_endpoint,
                    frame,
                )
                .await?
                {
                    return Ok(control);
                }
            }
            frame = connect_tunnel.recv() => {
                if let Some(control) = handle_connect_udp_tunnel_event(
                    runtime,
                    &listen_tunnel,
                    &connectors,
                    frame,
                )
                .await?
                {
                    return Ok(control);
                }
            }
        }
    }
}

async fn handle_listen_udp_tunnel_event(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    connectors: &UdpConnectorMap,
    connector_stream_ids: &mut StreamIdAllocator,
    connector_bind_endpoint: &str,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let frame = match recoverable_tunnel_frame(
        frame_result,
        "reading udp listen tunnel",
        "listen-side udp tunnel error",
        || async { Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Listen)) },
    )
    .await?
    {
        TunnelFrameOutcome::Frame(frame) => frame,
        TunnelFrameOutcome::Control(control) => return Ok(Some(control)),
    };
    match frame.frame_type {
        FrameType::UdpDatagram => {
            handle_listen_udp_datagram(
                runtime,
                connect_tunnel,
                connectors,
                connector_stream_ids,
                connector_bind_endpoint,
                frame,
            )
            .await
        }
        FrameType::Close => Ok(Some(ForwardLoopControl::Cancelled)),
        FrameType::Error if frame.stream_id == runtime.listen_session.listener_stream_id => {
            let meta = decode_tunnel_error_frame(&frame);
            if is_recoverable_pressure_tunnel_error(&meta) {
                runtime.record_dropped_datagram().await;
                return Ok(None);
            }
            Err(format_terminal_tunnel_error(&meta)).context("listen-side udp tunnel error")
        }
        FrameType::ForwardDrop => {
            apply_forward_drop_report(&runtime.store, runtime.forward_id().as_str(), &frame).await?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

async fn handle_listen_udp_datagram(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    connectors: &UdpConnectorMap,
    connector_stream_ids: &mut StreamIdAllocator,
    connector_bind_endpoint: &str,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let datagram: UdpDatagramMeta = decode_tunnel_meta(&frame)?;
    let connector_stream_id = match udp_connector_stream_id(
        connect_tunnel,
        connectors,
        connector_stream_ids,
        connector_bind_endpoint,
        datagram.peer.clone(),
        runtime.limits.max_udp_peers,
    )
    .await
    {
        Ok(stream_id) => stream_id,
        Err(UdpConnectorError::LimitExceeded) => {
            runtime.record_dropped_datagram().await;
            return Ok(None);
        }
        Err(UdpConnectorError::Transport(err)) => {
            runtime.record_dropped_datagram().await;
            return classify_transport_failure(
                err,
                "opening udp connector stream",
                TunnelRole::Connect,
            )
            .map(Some);
        }
    };
    if let Err(err) = connect_tunnel
        .send(Frame {
            frame_type: FrameType::UdpDatagram,
            flags: 0,
            stream_id: connector_stream_id,
            meta: encode_tunnel_meta(&UdpDatagramMeta {
                peer: runtime.connect_endpoint().to_string(),
            })?,
            data: frame.data,
        })
        .await
    {
        runtime.record_dropped_datagram().await;
        if is_backpressure_error(&err) {
            return Ok(None);
        }
        return classify_transport_failure(
            err,
            "relaying udp datagram to connect tunnel",
            TunnelRole::Connect,
        )
        .map(Some);
    }
    Ok(None)
}

async fn handle_connect_udp_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connectors: &UdpConnectorMap,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let frame = match recoverable_tunnel_frame(
        frame_result,
        "reading udp connect tunnel",
        "connect-side udp tunnel error",
        || async { Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)) },
    )
    .await?
    {
        TunnelFrameOutcome::Frame(frame) => frame,
        TunnelFrameOutcome::Control(control) => return Ok(Some(control)),
    };
    match frame.frame_type {
        FrameType::UdpBindOk => Ok(None),
        FrameType::Error => {
            runtime.record_dropped_datagram().await;
            remove_udp_connector(connectors, frame.stream_id).await;
            Ok(None)
        }
        FrameType::UdpDatagram => {
            handle_connect_udp_datagram(listen_tunnel, connectors, frame).await
        }
        FrameType::Close => {
            let _ = connectors.remove_by_stream_id(frame.stream_id).await;
            Ok(None)
        }
        FrameType::ForwardDrop => {
            // Public forward drop accounting is driven by the listen-side tunnel.
            // Connect-side UDP connector churn is peer-local cleanup only.
            Ok(None)
        }
        _ => Ok(None),
    }
}

async fn handle_connect_udp_datagram(
    listen_tunnel: &Arc<PortTunnel>,
    connectors: &UdpConnectorMap,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(peer) = connectors.peer_for_stream_id(frame.stream_id).await else {
        return Ok(None);
    };
    connectors
        .get_mut_by_peer(&peer, |connector| {
            connector.last_used = Instant::now();
        })
        .await;
    if let Err(err) = listen_tunnel
        .send(Frame {
            frame_type: FrameType::UdpDatagram,
            flags: 0,
            stream_id: 1,
            meta: encode_tunnel_meta(&UdpDatagramMeta { peer })?,
            data: frame.data,
        })
        .await
    {
        return classify_transport_failure(
            err,
            "relaying udp datagram to listen tunnel",
            TunnelRole::Listen,
        )
        .map(Some);
    }
    Ok(None)
}

async fn udp_connector_stream_id(
    connect_tunnel: &Arc<PortTunnel>,
    connectors: &UdpConnectorMap,
    connector_stream_ids: &mut StreamIdAllocator,
    connector_bind_endpoint: &str,
    peer: String,
    max_udp_peers: usize,
) -> Result<u32, UdpConnectorError> {
    if let Some(stream_id) = connectors
        .get_mut_by_peer(&peer, |connector| {
            connector.last_used = Instant::now();
            connector.stream_id
        })
        .await
    {
        return Ok(stream_id);
    }

    if connectors.len().await >= max_udp_peers {
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
    connectors
        .insert(
            peer,
            stream_id,
            UdpPeerConnector {
                stream_id,
                last_used: Instant::now(),
            },
        )
        .await;
    Ok(stream_id)
}

enum UdpConnectorError {
    LimitExceeded,
    Transport(anyhow::Error),
}

async fn remove_udp_connector(connectors: &UdpConnectorMap, stream_id: u32) {
    let _ = connectors.remove_by_stream_id(stream_id).await;
}

async fn sweep_idle_udp_connectors(connect_tunnel: &Arc<PortTunnel>, connectors: &UdpConnectorMap) {
    for (stream_id, _) in connectors
        .sweep_idle(Instant::now(), UDP_CONNECTOR_IDLE_TIMEOUT)
        .await
    {
        let _ = connect_tunnel.close_stream(stream_id).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use remote_exec_proto::port_forward::ForwardId;
    use remote_exec_proto::port_tunnel::{ForwardDropKind, ForwardDropMeta, write_frame};
    use remote_exec_proto::public::ForwardPortProtocol as PublicForwardPortProtocol;
    use remote_exec_proto::rpc::RpcErrorCode;
    use tokio_util::sync::CancellationToken;

    use super::super::epoch::{ForwardEpoch, INITIAL_FORWARD_GENERATION};
    use super::super::side::SideHandle;
    use super::super::supervisor::{
        ForwardIdentity, ForwardLimits, ForwardRuntime, ListenSessionControl,
    };
    use super::super::test_support::{
        ScriptedTunnelIo, filter_one, test_record, wait_until_send_fails,
    };
    use super::super::tunnel::PortTunnel;
    use super::*;

    fn udp_datagram_frame(stream_id: u32, peer: &str, data: &[u8]) -> Frame {
        Frame {
            frame_type: FrameType::UdpDatagram,
            flags: 0,
            stream_id,
            meta: encode_tunnel_meta(&UdpDatagramMeta {
                peer: peer.to_string(),
            })
            .unwrap(),
            data: data.to_vec(),
        }
    }

    #[tokio::test]
    async fn udp_connector_limit_refuses_new_peer_without_evicting_existing_peer() {
        let tunnel = SideHandle::local()
            .unwrap()
            .port_tunnel(PortTunnel::DEFAULT_MAX_QUEUED_BYTES)
            .await
            .unwrap();
        let connectors = UdpConnectorMap::default();

        const MAX_UDP_PEERS: usize = 3;
        for idx in 0..MAX_UDP_PEERS {
            let stream_id = (idx as u32 * 2) + 3;
            let last_used = Instant::now() - Duration::from_secs((idx + 1) as u64);
            let peer = format!("127.0.0.1:{}", 10_000 + idx);
            connectors
                .insert(
                    peer,
                    stream_id,
                    UdpPeerConnector {
                        stream_id,
                        last_used,
                    },
                )
                .await;
        }

        let mut connector_stream_ids = StreamIdAllocator::new_odd_from(9);
        let result = udp_connector_stream_id(
            &Arc::new(tunnel),
            &connectors,
            &mut connector_stream_ids,
            "127.0.0.1:0",
            "127.0.0.1:65535".to_string(),
            MAX_UDP_PEERS,
        )
        .await;
        assert!(matches!(result, Err(UdpConnectorError::LimitExceeded)));

        assert_eq!(connectors.len().await, MAX_UDP_PEERS);
        assert!(
            connectors
                .get_mut_by_peer("127.0.0.1:65535", |_| ())
                .await
                .is_none()
        );
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
            &udp_datagram_frame(1, "127.0.0.1:11001", b"first"),
        )
        .await
        .unwrap();

        let control = tokio::time::timeout(
            Duration::from_secs(1),
            run_udp_test_epoch(&runtime, listen_tunnel, connect_tunnel),
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
            async move { run_udp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &udp_datagram_frame(1, "127.0.0.1:11002", b"first"),
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
            &udp_datagram_frame(1, "127.0.0.1:11002", b"after-loss"),
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
            run_udp_test_epoch(&runtime, listen_tunnel, connect_tunnel),
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
            async move { run_udp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::Error,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "code": RpcErrorCode::PortTunnelLimitExceeded.wire_value(),
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
                let entries = runtime.store.list(&filter_one(runtime.forward_id().as_str())).await;
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
            async move { run_udp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::ForwardDrop,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&ForwardDropMeta::new(
                    ForwardDropKind::UdpDatagram,
                    3,
                    RpcErrorCode::PortTunnelLimitExceeded.wire_value(),
                    Some("port tunnel queued byte limit reached".to_string()),
                ))
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let entries = runtime.store.list(&filter_one(runtime.forward_id().as_str())).await;
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
        let initial_epoch = udp_test_epoch(listen_tunnel.clone(), connect_tunnel.clone());
        let listen_session = Arc::new(ListenSessionControl::new_for_test(
            SideHandle::local().unwrap(),
            ForwardId::new("fwd_test"),
            "test-session".to_string(),
            PublicForwardPortProtocol::Udp,
            Duration::from_secs(30),
            PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            Some(listen_tunnel),
        ));
        ForwardRuntime::new(
            ForwardIdentity::new(
                ForwardId::new("fwd_test"),
                SideHandle::local().unwrap(),
                SideHandle::local().unwrap(),
                PublicForwardPortProtocol::Udp,
                "127.0.0.1:1".to_string(),
            ),
            ForwardLimits::default(),
            Default::default(),
            listen_session,
            initial_epoch,
            CancellationToken::new(),
        )
    }

    fn udp_test_epoch(
        listen_tunnel: Arc<PortTunnel>,
        connect_tunnel: Arc<PortTunnel>,
    ) -> ForwardEpoch {
        ForwardEpoch::new(INITIAL_FORWARD_GENERATION, listen_tunnel, connect_tunnel)
    }

    async fn run_udp_test_epoch(
        runtime: &ForwardRuntime,
        listen_tunnel: Arc<PortTunnel>,
        connect_tunnel: Arc<PortTunnel>,
    ) -> anyhow::Result<ForwardLoopControl> {
        let epoch = udp_test_epoch(listen_tunnel, connect_tunnel);
        run_udp_forward_epoch(runtime, &epoch).await
    }
}
