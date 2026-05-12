mod codec;
mod error;
mod limiter;
mod session;
mod session_store;
mod tcp;
mod timings;
mod tunnel;
mod types;
mod udp;

use remote_exec_proto::port_tunnel::{
    ForwardDropKind, ForwardDropMeta, Frame, FrameType, HEADER_LEN,
};
use remote_exec_proto::rpc::RpcErrorCode;

pub use session_store::TunnelSessionStore;
pub use tunnel::{reserve_tunnel_connection, serve_tunnel, serve_tunnel_with_permit};

pub use limiter::{PortForwardLimiter, PortForwardPermit};
use timings::timings;
use types::{
    EndpointMeta, EndpointOkMeta, ErrorMeta, QueuedFrame, TcpAcceptMeta, TcpStreamEntry,
    TcpWriteCommand, TcpWriterHandle, TransportUdpBind, TunnelMode, TunnelSender, TunnelState,
    UdpDatagramMeta, UdpReaderEntry,
};

const READ_BUF_SIZE: usize = 64 * 1024;
const TCP_WRITE_QUEUE_FRAMES: usize = 8;

impl TunnelState {
    async fn send(&self, frame: Frame) -> Result<(), crate::HostRpcError> {
        self.tx.send(frame).await
    }
}

impl TunnelSender {
    async fn send(&self, frame: Frame) -> Result<(), crate::HostRpcError> {
        let permit = self.limiter.try_acquire_queued_frame(&frame)?;
        let queued = QueuedFrame {
            frame,
            _permit: permit,
        };
        self.tx.send(queued).await.map_err(|_| {
            error::rpc_error(
                RpcErrorCode::PortTunnelClosed,
                "port tunnel writer is closed",
            )
        })
    }
}

fn queued_frame_charge(frame: &Frame) -> usize {
    if frame.data.is_empty() {
        0
    } else {
        HEADER_LEN
            .saturating_add(frame.meta.len())
            .saturating_add(frame.data.len())
    }
}

async fn send_forward_drop_report(
    tx: &TunnelSender,
    stream_id: u32,
    kind: ForwardDropKind,
    reason: impl Into<String>,
    message: impl Into<String>,
) -> Result<(), crate::HostRpcError> {
    let meta = serde_json::to_vec(&ForwardDropMeta {
        kind,
        count: 1,
        reason: reason.into(),
        message: Some(message.into()),
    })
    .map_err(|err| error::rpc_error(RpcErrorCode::InvalidPortTunnel, err.to_string()))?;
    tx.send(Frame {
        frame_type: FrameType::ForwardDrop,
        flags: 0,
        stream_id,
        meta,
        data: Vec::new(),
    })
    .await
}

#[cfg(test)]
mod port_tunnel_tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, MAX_DATA_LEN, TunnelForwardProtocol, TunnelOpenMeta, TunnelReadyMeta,
        TunnelRole, read_frame, write_frame, write_preface,
    };
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    use super::tcp::{tunnel_close_stream, tunnel_tcp_eof};
    use super::*;
    use crate::{
        AppState, HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig,
        build_runtime_state,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_LOOPBACK_HOST: AtomicU32 = AtomicU32::new(1);

    #[tokio::test]
    async fn tunnel_binds_tcp_listener_and_releases_it_on_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (mut broker_side, ready) =
            start_open_listen_tunnel_with_ready(state.clone(), TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": listen_endpoint }),
            ),
        )
        .await
        .unwrap();

        let ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ok.frame_type, FrameType::TcpListenOk);
        drop(broker_side);

        wait_until_session_removed(&state, &session_id_from_ready(&ready)).await;
    }

    #[tokio::test]
    async fn tunnel_tcp_connect_echoes_binary_data_full_duplex() {
        let state = test_state();
        let echo_endpoint = spawn_tcp_echo_server().await;
        let mut broker_side = start_open_connect_tunnel(state, TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": echo_endpoint }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        write_frame(
            &mut broker_side,
            &data_frame(FrameType::TcpData, 1, b"\0hello\xff".to_vec()),
        )
        .await
        .unwrap();
        let echoed = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(echoed.frame_type, FrameType::TcpData);
        assert_eq!(echoed.data, b"\0hello\xff");
    }

    #[tokio::test]
    async fn tunnel_tcp_data_write_pressure_does_not_block_control_frames() {
        let state = test_state();
        let endpoint = spawn_tcp_non_draining_server().await;
        let (broker_side, daemon_side) = tokio::io::duplex(32 * 1024 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));
        let (mut broker_read, mut broker_write) = tokio::io::split(broker_side);

        write_preface(&mut broker_write).await.unwrap();
        write_tunnel_open_on_writer(
            &mut broker_write,
            TunnelRole::Connect,
            TunnelForwardProtocol::Tcp,
            None,
        )
        .await;
        assert_eq!(
            read_frame(&mut broker_read).await.unwrap().frame_type,
            FrameType::TunnelReady
        );
        write_frame(
            &mut broker_write,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_read).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        let heartbeat_meta = br#"{"nonce":1}"#.to_vec();
        let writer = tokio::spawn({
            let heartbeat_meta = heartbeat_meta.clone();
            async move {
                let payload = vec![0x7b; MAX_DATA_LEN];
                for _ in 0..64 {
                    write_frame(
                        &mut broker_write,
                        &data_frame(FrameType::TcpData, 1, payload.clone()),
                    )
                    .await
                    .unwrap();
                }
                write_frame(
                    &mut broker_write,
                    &Frame {
                        frame_type: FrameType::TunnelHeartbeat,
                        flags: 0,
                        stream_id: 0,
                        meta: heartbeat_meta,
                        data: Vec::new(),
                    },
                )
                .await
                .unwrap();
            }
        });

        let ack = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let frame = read_frame(&mut broker_read).await.unwrap();
                if frame.frame_type == FrameType::TunnelHeartbeatAck {
                    return frame;
                }
            }
        })
        .await
        .expect("tcp data writes must not block tunnel control frames");
        writer.abort();
        assert_eq!(ack.meta, heartbeat_meta);
    }

    #[tokio::test]
    async fn tcp_write_queue_limit_releases_active_stream_capacity() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_active_tcp_streams: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let pressured_endpoint = spawn_tcp_non_draining_server().await;
        let replacement_endpoint = spawn_tcp_hold_server().await;
        let (broker_side, daemon_side) = tokio::io::duplex(32 * 1024 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));
        let (mut broker_read, mut broker_write) = tokio::io::split(broker_side);

        write_preface(&mut broker_write).await.unwrap();
        write_tunnel_open_on_writer(
            &mut broker_write,
            TunnelRole::Connect,
            TunnelForwardProtocol::Tcp,
            None,
        )
        .await;
        assert_eq!(
            read_frame(&mut broker_read).await.unwrap().frame_type,
            FrameType::TunnelReady
        );
        write_frame(
            &mut broker_write,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": pressured_endpoint }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_read).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        let writer = tokio::spawn(async move {
            let payload = vec![0x33; MAX_DATA_LEN];
            for _ in 0..64 {
                write_frame(
                    &mut broker_write,
                    &data_frame(FrameType::TcpData, 1, payload.clone()),
                )
                .await
                .unwrap();
            }
            write_frame(
                &mut broker_write,
                &json_frame(
                    FrameType::TcpConnect,
                    3,
                    serde_json::json!({ "endpoint": replacement_endpoint }),
                ),
            )
            .await
            .unwrap();
        });

        let mut saw_queue_limit = false;
        let mut saw_replacement_connect = false;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let frame = read_frame(&mut broker_read).await.unwrap();
                match frame.frame_type {
                    FrameType::Error if frame.stream_id == 1 => {
                        let meta = serde_json::from_slice::<Value>(&frame.meta).unwrap();
                        if meta["code"]
                            == remote_exec_proto::rpc::RpcErrorCode::PortTunnelLimitExceeded
                                .wire_value()
                        {
                            saw_queue_limit = true;
                        }
                    }
                    FrameType::TcpConnectOk if frame.stream_id == 3 => {
                        saw_replacement_connect = true;
                    }
                    _ => {}
                }
                if saw_queue_limit && saw_replacement_connect {
                    return;
                }
            }
        })
        .await
        .expect("queue-full stream should release active stream capacity");
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn tunnel_udp_bind_relays_datagrams_from_two_peers() {
        let state = test_state();
        let endpoint = free_loopback_endpoint();
        let mut broker_side = start_open_connect_tunnel(state, TunnelForwardProtocol::Udp).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let bind_ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(bind_ok.frame_type, FrameType::UdpBindOk);
        let bound_endpoint = endpoint_from_frame(&bind_ok);

        let peer_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer_a.send_to(b"from-a", &bound_endpoint).await.unwrap();
        peer_b.send_to(b"from-b", &bound_endpoint).await.unwrap();

        let first = read_frame(&mut broker_side).await.unwrap();
        let second = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(first.frame_type, FrameType::UdpDatagram);
        assert_eq!(second.frame_type, FrameType::UdpDatagram);
        assert_eq!(
            sorted_payloads([first.data, second.data]),
            vec![b"from-a".to_vec(), b"from-b".to_vec()]
        );
    }

    #[tokio::test]
    async fn tcp_listener_session_can_resume_after_transport_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (listen_bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id, TunnelForwardProtocol::Tcp).await;
        let accept = tokio::net::TcpStream::connect(&listen_bound_endpoint)
            .await
            .unwrap();
        drop(accept);

        let frame = read_frame(&mut resumed).await.unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpAccept);
    }

    #[tokio::test]
    async fn resumed_tcp_listener_session_closes_with_retained_generation() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (_listen_bound_endpoint, session_id) =
            open_v4_resumable_tcp_listener(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id, TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut resumed,
            &json_frame(
                FrameType::TunnelClose,
                0,
                serde_json::json!({
                    "forward_id": "fwd_test",
                    "generation": 1,
                    "reason": "operator_close"
                }),
            ),
        )
        .await
        .unwrap();

        let frame = read_frame(&mut resumed).await.unwrap();
        assert_eq!(frame.frame_type, FrameType::TunnelClosed);
    }

    #[tokio::test]
    async fn udp_bind_session_can_resume_after_transport_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (listen_bound_endpoint, session_id) =
            open_resumable_udp_bind(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id, TunnelForwardProtocol::Udp).await;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(b"ping", &listen_bound_endpoint)
            .await
            .unwrap();

        let frame = read_frame(&mut resumed).await.unwrap();
        assert_eq!(frame.frame_type, FrameType::UdpDatagram);
        assert_eq!(frame.data, b"ping");
    }

    #[tokio::test]
    async fn expired_detached_listener_is_released() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (_bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        tokio::time::sleep(Duration::from_millis(250)).await;

        wait_until_session_removed(&state, &session_id).await;
    }

    #[tokio::test]
    async fn detached_tcp_listener_accepts_after_resume_not_before() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        let client = tokio::net::TcpStream::connect(&bound_endpoint)
            .await
            .unwrap();
        let mut resumed = resume_session(&state, &session_id, TunnelForwardProtocol::Tcp).await;

        let frame = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut resumed))
            .await
            .expect("accept should arrive after resume")
            .unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpAccept);

        drop(client);
    }

    #[tokio::test]
    async fn host_shutdown_releases_detached_tcp_listener_without_resume_timeout() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (mut broker_side, ready) =
            start_open_listen_tunnel_with_ready(state.clone(), TunnelForwardProtocol::Tcp).await;
        let session_id = session_id_from_ready(&ready);
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": listen_endpoint }),
            ),
        )
        .await
        .unwrap();
        let ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ok.frame_type, FrameType::TcpListenOk);

        state.shutdown.cancel();
        drop(broker_side);

        tokio::time::timeout(
            Duration::from_millis(100),
            wait_until_session_removed(&state, &session_id),
        )
        .await
        .expect("shutdown should release retained listener promptly");
    }

    #[tokio::test]
    async fn tunnel_reports_tcp_listen_errors_on_request_stream() {
        let state = test_state();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_endpoint = occupied.local_addr().unwrap().to_string();
        let mut broker_side = start_open_listen_tunnel(state, TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                7,
                serde_json::json!({ "endpoint": occupied_endpoint }),
            ),
        )
        .await
        .unwrap();

        let error = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut broker_side))
            .await
            .expect("listen error frame should arrive")
            .unwrap();
        assert_eq!(error.frame_type, FrameType::Error);
        assert_eq!(error.stream_id, 7);
    }

    #[tokio::test]
    async fn tunnel_exits_promptly_when_host_shuts_down() {
        let state = test_state();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        let tunnel_task = tokio::spawn(serve_tunnel(state.clone(), daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        state.shutdown.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), tunnel_task)
            .await
            .expect("tunnel should exit after host shutdown")
            .unwrap();
        result.unwrap();
    }

    #[tokio::test]
    async fn background_task_tracker_joins_shutdown_tunnel() {
        let state = test_state();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        let tracker = state.background_tasks.clone();
        let tunnel_state = state.clone();
        tracker
            .spawn("test port-forward tunnel", async move {
                serve_tunnel(tunnel_state, daemon_side)
                    .await
                    .map_err(|err| anyhow::anyhow!("{}: {}", err.code, err.message))
            })
            .await;

        write_preface(&mut broker_side).await.unwrap();
        state.shutdown.cancel();

        tokio::time::timeout(Duration::from_secs(1), tracker.join_all())
            .await
            .expect("tracked tunnel should join after host shutdown");
    }

    #[tokio::test]
    async fn tunnel_ready_reports_configured_limits() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_active_tcp_streams: 3,
            max_udp_binds: 5,
            max_tunnel_queued_bytes: 4096,
            connect_timeout_ms: 7_000,
            ..crate::HostPortForwardLimits::default()
        });
        assert_eq!(state.config.port_forward_limits.connect_timeout_ms, 7_000);
        let mut broker_side = start_tunnel(state).await;

        write_frame(
            &mut broker_side,
            &Frame {
                frame_type: FrameType::TunnelOpen,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&TunnelOpenMeta {
                    forward_id: "fwd_limits".to_string(),
                    role: TunnelRole::Connect,
                    side: "test".to_string(),
                    generation: 7,
                    protocol: TunnelForwardProtocol::Tcp,
                    resume_session_id: None,
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        let ready = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ready.frame_type, FrameType::TunnelReady);
        let meta: TunnelReadyMeta = serde_json::from_slice(&ready.meta).unwrap();
        assert_eq!(meta.generation, 7);
        assert_eq!(meta.limits.max_active_tcp_streams, 3);
        assert_eq!(meta.limits.max_udp_peers, 5);
        assert_eq!(meta.limits.max_queued_bytes, 4096);
    }

    #[tokio::test]
    async fn legacy_session_frames_are_reserved_but_unsupported() {
        for frame_type in [FrameType::SessionOpen, FrameType::SessionResume] {
            let state = test_state();
            let mut broker_side = start_tunnel(state).await;
            write_frame(
                &mut broker_side,
                &json_frame(
                    frame_type,
                    0,
                    serde_json::json!({ "session_id": "legacy_session" }),
                ),
            )
            .await
            .unwrap();

            let error = read_frame(&mut broker_side).await.unwrap();
            assert_eq!(error.frame_type, FrameType::Error);
            assert_eq!(error.stream_id, 0);
            let meta: serde_json::Value = serde_json::from_slice(&error.meta).unwrap();
            assert_eq!(meta["code"], "invalid_port_tunnel");
        }
    }

    #[tokio::test]
    async fn tunnel_rejects_frames_for_wrong_protocol() {
        let state = test_state();
        let mut tcp_connect =
            start_open_connect_tunnel(state.clone(), TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut tcp_connect,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": "127.0.0.1:0" }),
            ),
        )
        .await
        .unwrap();
        let error = read_frame(&mut tcp_connect).await.unwrap();
        assert_error_code(error, 1, "invalid_port_tunnel");

        let mut udp_listen = start_open_listen_tunnel(state, TunnelForwardProtocol::Udp).await;
        write_frame(
            &mut udp_listen,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": "127.0.0.1:0" }),
            ),
        )
        .await
        .unwrap();
        let error = read_frame(&mut udp_listen).await.unwrap();
        assert_error_code(error, 1, "invalid_port_tunnel");
    }

    #[tokio::test]
    async fn retained_tcp_listener_limit_rejects_second_session_listener() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_retained_sessions: 2,
            max_retained_listeners: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let first_endpoint = free_loopback_endpoint();
        let second_endpoint = free_loopback_endpoint();
        let (_first_bound, _first_session_id) =
            open_resumable_tcp_listener(&state, &first_endpoint).await;

        let mut second = start_open_listen_tunnel(state, TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut second,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": second_endpoint }),
            ),
        )
        .await
        .unwrap();

        let error = read_frame(&mut second).await.unwrap();
        assert_limit_error(error, 1);
    }

    #[tokio::test]
    async fn udp_bind_limit_rejects_second_bind() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_udp_binds: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let mut broker_side = start_open_connect_tunnel(state, TunnelForwardProtocol::Udp).await;

        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": "127.0.0.1:0" }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::UdpBindOk
        );

        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                3,
                serde_json::json!({ "endpoint": "127.0.0.1:0" }),
            ),
        )
        .await
        .unwrap();

        let error = read_frame(&mut broker_side).await.unwrap();
        assert_limit_error(error, 3);
    }

    #[tokio::test]
    async fn active_tcp_stream_limit_rejects_second_connect() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_active_tcp_streams: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let destination = spawn_tcp_hold_server().await;
        let mut broker_side = start_open_connect_tunnel(state, TunnelForwardProtocol::Tcp).await;

        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": destination }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpConnect,
                3,
                serde_json::json!({ "endpoint": destination }),
            ),
        )
        .await
        .unwrap();

        let error = read_frame(&mut broker_side).await.unwrap();
        assert_limit_error(error, 3);
    }

    #[tokio::test]
    async fn active_tcp_accept_limit_drops_accept_without_listener_error() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_active_tcp_streams: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let destination = spawn_tcp_hold_server().await;
        let mut connect_side =
            start_open_connect_tunnel(state.clone(), TunnelForwardProtocol::Tcp).await;

        write_frame(
            &mut connect_side,
            &json_frame(
                FrameType::TcpConnect,
                3,
                serde_json::json!({ "endpoint": destination }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut connect_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        let mut broker_side = start_open_listen_tunnel(state, TunnelForwardProtocol::Tcp).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": "127.0.0.1:0" }),
            ),
        )
        .await
        .unwrap();
        let listen_ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(listen_ok.frame_type, FrameType::TcpListenOk);
        let bound_endpoint = endpoint_from_frame(&listen_ok);

        let refused = tokio::net::TcpStream::connect(&bound_endpoint)
            .await
            .unwrap();
        drop(refused);

        let drop_report = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let frame = read_frame(&mut broker_side).await.unwrap();
                if frame.frame_type == FrameType::ForwardDrop {
                    return frame;
                }
            }
        })
        .await
        .expect("listener pressure should emit drop telemetry");
        let meta: ForwardDropMeta = serde_json::from_slice(&drop_report.meta).unwrap();
        assert_eq!(meta.kind, ForwardDropKind::TcpStream);
        assert_eq!(meta.count, 1);
        assert_eq!(meta.reason, "port_tunnel_limit_exceeded");
    }

    #[tokio::test]
    async fn tunnel_connection_limit_rejects_second_concurrent_tunnel() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_tunnel_connections: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let _first = start_tunnel(state.clone()).await;
        let (_second_broker, second_daemon) = tokio::io::duplex(64 * 1024);
        let second_task = tokio::spawn(serve_tunnel(state, second_daemon));

        let result = tokio::time::timeout(Duration::from_secs(1), second_task)
            .await
            .expect("second tunnel should be rejected promptly")
            .unwrap();
        let error = result.expect_err("second tunnel should exceed the connection limit");
        assert_eq!(error.code, "port_tunnel_limit_exceeded");
    }

    #[tokio::test]
    async fn queued_byte_limit_reports_backpressure_error() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_tunnel_queued_bytes: 128,
            ..crate::HostPortForwardLimits::default()
        });
        let destination = spawn_tcp_one_shot_sender(vec![42u8; 512]).await;
        let mut broker_side = start_open_connect_tunnel(state, TunnelForwardProtocol::Tcp).await;

        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": destination }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        let error = read_frame(&mut broker_side).await.unwrap();
        assert_limit_error(error, 1);
    }

    #[tokio::test]
    async fn tcp_close_after_peer_eof_does_not_cancel_queued_writer_shutdown() {
        let state = test_state();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<QueuedFrame>(8);
        let stream_cancel = state.shutdown.child_token();
        let (writer_tx, mut writer_rx) = tokio::sync::mpsc::channel(TCP_WRITE_QUEUE_FRAMES);
        let tunnel = Arc::new(TunnelState {
            state: state.clone(),
            cancel: state.shutdown.child_token(),
            tx: TunnelSender {
                tx,
                limiter: state.port_forward_limiter.clone(),
            },
            open_mode: tokio::sync::Mutex::new(TunnelMode::Connect {
                protocol: TunnelForwardProtocol::Tcp,
            }),
            tcp_streams: tokio::sync::Mutex::new(HashMap::from([(
                1,
                TcpStreamEntry {
                    writer: TcpWriterHandle {
                        tx: writer_tx,
                        cancel: stream_cancel.clone(),
                    },
                    _permit: state
                        .port_forward_limiter
                        .try_acquire_active_tcp_stream()
                        .unwrap(),
                    cancel: Some(stream_cancel.clone()),
                },
            )])),
            udp_binds: tokio::sync::Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            attached_session: tokio::sync::Mutex::new(None),
            _connection_permit: state
                .port_forward_limiter
                .try_acquire_tunnel_connection()
                .unwrap(),
        });

        tunnel_tcp_eof(&tunnel, 1).await.unwrap();
        tunnel_close_stream(&tunnel, 1).await.unwrap();

        assert!(
            !stream_cancel.is_cancelled(),
            "cleanup Close after peer EOF must not abort the queued writer shutdown"
        );
        let Some(TcpWriteCommand::Shutdown) = writer_rx.recv().await else {
            panic!("expected writer shutdown command");
        };
        let close = rx.recv().await.expect("close frame").frame;
        assert_eq!(close.frame_type, FrameType::Close);
        assert_eq!(close.stream_id, 1);
    }

    #[tokio::test]
    async fn tunnel_tcp_eof_waits_for_full_writer_queue() {
        let state = test_state();
        let (tx, _rx) = tokio::sync::mpsc::channel::<QueuedFrame>(8);
        let stream_cancel = state.shutdown.child_token();
        let (writer_tx, mut writer_rx) = tokio::sync::mpsc::channel(TCP_WRITE_QUEUE_FRAMES);
        for _ in 0..TCP_WRITE_QUEUE_FRAMES {
            writer_tx
                .send(TcpWriteCommand::Data(Vec::new()))
                .await
                .unwrap();
        }
        let tunnel = Arc::new(TunnelState {
            state: state.clone(),
            cancel: state.shutdown.child_token(),
            tx: TunnelSender {
                tx,
                limiter: state.port_forward_limiter.clone(),
            },
            open_mode: tokio::sync::Mutex::new(TunnelMode::Connect {
                protocol: TunnelForwardProtocol::Tcp,
            }),
            tcp_streams: tokio::sync::Mutex::new(HashMap::from([(
                1,
                TcpStreamEntry {
                    writer: TcpWriterHandle {
                        tx: writer_tx,
                        cancel: stream_cancel.clone(),
                    },
                    _permit: state
                        .port_forward_limiter
                        .try_acquire_active_tcp_stream()
                        .unwrap(),
                    cancel: Some(stream_cancel.clone()),
                },
            )])),
            udp_binds: tokio::sync::Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            attached_session: tokio::sync::Mutex::new(None),
            _connection_permit: state
                .port_forward_limiter
                .try_acquire_tunnel_connection()
                .unwrap(),
        });

        let eof = tokio::spawn({
            let tunnel = tunnel.clone();
            async move { tunnel_tcp_eof(&tunnel, 1).await.unwrap() }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !eof.is_finished(),
            "EOF should wait instead of dropping shutdown"
        );

        let Some(TcpWriteCommand::Data(_)) = writer_rx.recv().await else {
            panic!("expected queued data before shutdown");
        };
        tokio::time::timeout(Duration::from_secs(1), eof)
            .await
            .expect("EOF should complete after queue has capacity")
            .unwrap();
        while let Some(command) = writer_rx.recv().await {
            if matches!(command, TcpWriteCommand::Shutdown) {
                return;
            }
        }
        panic!("expected shutdown command");
    }

    #[tokio::test]
    async fn concurrent_tunnel_open_allows_only_one_mode() {
        let state = test_state();
        let retained_session = super::tunnel::new_session_for_test(&state);
        state
            .port_forward_sessions
            .try_insert(
                retained_session.clone(),
                state.config.port_forward_limits.max_retained_sessions,
            )
            .await
            .unwrap();
        let attachment_lock = retained_session.attachment.lock().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<QueuedFrame>(8);
        let tunnel = Arc::new(TunnelState {
            state: state.clone(),
            cancel: state.shutdown.child_token(),
            tx: TunnelSender {
                tx,
                limiter: state.port_forward_limiter.clone(),
            },
            open_mode: tokio::sync::Mutex::new(TunnelMode::Unopened),
            tcp_streams: tokio::sync::Mutex::new(HashMap::new()),
            udp_binds: tokio::sync::Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            attached_session: tokio::sync::Mutex::new(None),
            _connection_permit: state
                .port_forward_limiter
                .try_acquire_tunnel_connection()
                .unwrap(),
        });
        let listen_open = Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelOpenMeta {
                forward_id: "forward-a".to_string(),
                role: TunnelRole::Listen,
                side: "test".to_string(),
                generation: 1,
                protocol: TunnelForwardProtocol::Tcp,
                resume_session_id: Some(retained_session.id.clone()),
            })
            .unwrap(),
            data: Vec::new(),
        };

        let first = tokio::spawn({
            let tunnel = tunnel.clone();
            let frame = listen_open.clone();
            async move { super::tunnel::handle_tunnel_frame(tunnel, frame).await }
        });
        let second = tokio::spawn({
            let tunnel = tunnel.clone();
            async move { super::tunnel::handle_tunnel_frame(tunnel, listen_open).await }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(attachment_lock);

        let first = first.await.unwrap();
        let second = second.await.unwrap();
        let success_count = usize::from(first.is_ok()) + usize::from(second.is_ok());
        let already_open_count = [first, second]
            .into_iter()
            .filter(|result| {
                result.as_ref().is_err_and(|err| {
                    err.code == RpcErrorCode::PortTunnelAlreadyAttached.wire_value()
                })
            })
            .count();
        assert_eq!(success_count, 1);
        assert_eq!(already_open_count, 1);

        let ready_count = {
            let mut count = 0;
            while let Ok(queued) = rx.try_recv() {
                if queued.frame.frame_type == FrameType::TunnelReady {
                    count += 1;
                }
            }
            count
        };
        assert_eq!(ready_count, 1);
        assert_eq!(state.port_forward_sessions.sessions.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn reattached_session_is_not_removed_by_stale_expiry() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint();
        let (_bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;
        let session = state
            .port_forward_sessions
            .get(&session_id)
            .await
            .expect("detached session should be retained");
        wait_until_session_has_expiry_task(&session).await;

        let _resumed = resume_session(&state, &session_id, TunnelForwardProtocol::Tcp).await;
        assert!(
            !session.has_expiry_task().await,
            "reattach should cancel stale expiry task"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(
            state.port_forward_sessions.get(&session_id).await.is_some(),
            "stale expiry task must not remove a reattached session"
        );
    }

    #[tokio::test]
    async fn retained_udp_queued_byte_pressure_reports_drop() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_tunnel_queued_bytes: 128,
            ..crate::HostPortForwardLimits::default()
        });
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &Frame {
                frame_type: FrameType::TunnelOpen,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&TunnelOpenMeta {
                    forward_id: "fwd_test".to_string(),
                    role: TunnelRole::Listen,
                    side: "test".to_string(),
                    generation: 1,
                    protocol: TunnelForwardProtocol::Udp,
                    resume_session_id: None,
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TunnelReady
        );
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": "127.0.0.1:0" }),
            ),
        )
        .await
        .unwrap();
        let bind_ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(bind_ok.frame_type, FrameType::UdpBindOk);
        let bound_endpoint = endpoint_from_frame(&bind_ok);

        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(&vec![42u8; 512], &bound_endpoint)
            .await
            .unwrap();

        let drop_report = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let frame = read_frame(&mut broker_side).await.unwrap();
                if frame.frame_type == FrameType::ForwardDrop {
                    return frame;
                }
            }
        })
        .await
        .expect("udp pressure should emit drop telemetry");
        let meta: ForwardDropMeta = serde_json::from_slice(&drop_report.meta).unwrap();
        assert_eq!(meta.kind, ForwardDropKind::UdpDatagram);
        assert_eq!(meta.count, 1);
        assert_eq!(meta.reason, "port_tunnel_limit_exceeded");
    }

    fn test_state() -> Arc<AppState> {
        test_state_with_limits(crate::HostPortForwardLimits::default())
    }

    fn test_state_with_limits(port_forward_limits: crate::HostPortForwardLimits) -> Arc<AppState> {
        let workdir = std::env::temp_dir().join(format!(
            "remote-exec-host-port-tunnel-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workdir).unwrap();
        Arc::new(
            build_runtime_state(HostRuntimeConfig {
                target: "test".to_string(),
                default_workdir: workdir,
                windows_posix_root: None,
                sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                max_open_sessions: crate::config::DEFAULT_MAX_OPEN_SESSIONS,
                allow_login_shell: true,
                pty: PtyMode::None,
                default_shell: None,
                yield_time: YieldTimeConfig::default(),
                port_forward_limits,
                experimental_apply_patch_target_encoding_autodetect: false,
                process_environment: ProcessEnvironment::capture_current(),
            })
            .unwrap(),
        )
    }

    async fn start_tunnel(state: Arc<AppState>) -> DuplexStream {
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));
        write_preface(&mut broker_side).await.unwrap();
        broker_side
    }

    async fn start_open_listen_tunnel(
        state: Arc<AppState>,
        protocol: TunnelForwardProtocol,
    ) -> DuplexStream {
        start_open_listen_tunnel_with_ready(state, protocol).await.0
    }

    async fn start_open_listen_tunnel_with_ready(
        state: Arc<AppState>,
        protocol: TunnelForwardProtocol,
    ) -> (DuplexStream, Frame) {
        let mut broker_side = start_tunnel(state).await;
        write_tunnel_open(&mut broker_side, TunnelRole::Listen, protocol, None).await;
        let ready = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ready.frame_type, FrameType::TunnelReady);
        (broker_side, ready)
    }

    async fn start_open_connect_tunnel(
        state: Arc<AppState>,
        protocol: TunnelForwardProtocol,
    ) -> DuplexStream {
        let mut broker_side = start_tunnel(state).await;
        write_tunnel_open(&mut broker_side, TunnelRole::Connect, protocol, None).await;
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TunnelReady
        );
        broker_side
    }

    async fn write_tunnel_open(
        writer: &mut DuplexStream,
        role: TunnelRole,
        protocol: TunnelForwardProtocol,
        resume_session_id: Option<String>,
    ) {
        write_tunnel_open_on_writer(writer, role, protocol, resume_session_id).await;
    }

    async fn write_tunnel_open_on_writer<W>(
        writer: &mut W,
        role: TunnelRole,
        protocol: TunnelForwardProtocol,
        resume_session_id: Option<String>,
    ) where
        W: tokio::io::AsyncWrite + Unpin,
    {
        write_frame(
            writer,
            &Frame {
                frame_type: FrameType::TunnelOpen,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&TunnelOpenMeta {
                    forward_id: "fwd_test".to_string(),
                    role,
                    side: "test".to_string(),
                    generation: 1,
                    protocol,
                    resume_session_id,
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
    }

    async fn open_resumable_tcp_listener(
        state: &Arc<AppState>,
        endpoint: &str,
    ) -> (String, String) {
        let (mut broker_side, ready) =
            start_open_listen_tunnel_with_ready(state.clone(), TunnelForwardProtocol::Tcp).await;
        let session_id = session_id_from_ready(&ready);
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let ok = read_frame(&mut broker_side).await.unwrap();
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);
        (bound_endpoint, session_id)
    }

    async fn open_v4_resumable_tcp_listener(
        state: &Arc<AppState>,
        endpoint: &str,
    ) -> (String, String) {
        open_resumable_tcp_listener(state, endpoint).await
    }

    async fn open_resumable_udp_bind(state: &Arc<AppState>, endpoint: &str) -> (String, String) {
        let (mut broker_side, ready) =
            start_open_listen_tunnel_with_ready(state.clone(), TunnelForwardProtocol::Udp).await;
        let session_id = session_id_from_ready(&ready);
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let ok = read_frame(&mut broker_side).await.unwrap();
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);
        (bound_endpoint, session_id)
    }

    async fn resume_session(
        state: &Arc<AppState>,
        session_id: &str,
        protocol: TunnelForwardProtocol,
    ) -> DuplexStream {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_tunnel_open(
            &mut broker_side,
            TunnelRole::Listen,
            protocol,
            Some(session_id.to_string()),
        )
        .await;
        let ready = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ready.frame_type, FrameType::TunnelReady);
        broker_side
    }

    fn session_id_from_ready(frame: &Frame) -> String {
        serde_json::from_slice::<Value>(&frame.meta).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn free_loopback_endpoint() -> String {
        #[cfg(windows)]
        {
            free_windows_loopback_endpoint()
        }
        #[cfg(not(windows))]
        {
            unique_loopback_bind_endpoint()
        }
    }

    #[cfg(windows)]
    fn free_windows_loopback_endpoint() -> String {
        for _ in 0..512 {
            let value = NEXT_LOOPBACK_HOST.fetch_add(1, Ordering::Relaxed);
            let third_octet = (value / 254) % 254 + 1;
            let fourth_octet = value % 254 + 1;
            let port = 700 + (value % 300);
            let endpoint = format!("127.42.{third_octet}.{fourth_octet}:{port}");
            let Ok(tcp_listener) = std::net::TcpListener::bind(&endpoint) else {
                continue;
            };
            let Ok(udp_socket) = std::net::UdpSocket::bind(&endpoint) else {
                drop(tcp_listener);
                continue;
            };
            drop(udp_socket);
            drop(tcp_listener);
            return endpoint;
        }
        panic!("could not find a free low loopback endpoint");
    }

    #[cfg(not(windows))]
    fn unique_loopback_bind_endpoint() -> String {
        let value = NEXT_LOOPBACK_HOST.fetch_add(1, Ordering::Relaxed);
        let third_octet = (value / 254) % 254 + 1;
        let fourth_octet = value % 254 + 1;
        format!("127.42.{third_octet}.{fourth_octet}:0")
    }

    async fn wait_until_session_removed(state: &AppState, session_id: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if !state.port_forward_sessions.contains(session_id).await {
                return;
            }
            if let Some(session) = state.port_forward_sessions.get(session_id).await
                && !session.has_retained_listener().await
            {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("session `{session_id}` retained listener did not close");
    }

    async fn wait_until_session_has_expiry_task(session: &super::session::SessionState) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            if session.has_expiry_task().await {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "detached session should schedule an expiry task"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn spawn_tcp_echo_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
            let read = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..read]).await.unwrap();
        });
        endpoint
    }

    async fn spawn_tcp_hold_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(value) => value,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let mut buf = [0; 1];
                    let _ = stream.read(&mut buf).await;
                });
            }
        });
        endpoint
    }

    async fn spawn_tcp_non_draining_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            set_small_tcp_recv_buffer(&stream);
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(stream);
        });
        endpoint
    }

    #[cfg(unix)]
    fn set_small_tcp_recv_buffer(stream: &tokio::net::TcpStream) {
        use std::mem;
        use std::os::fd::AsRawFd;

        let size: nix::libc::c_int = 1024;
        let rc = unsafe {
            nix::libc::setsockopt(
                stream.as_raw_fd(),
                nix::libc::SOL_SOCKET,
                nix::libc::SO_RCVBUF,
                &size as *const _ as *const nix::libc::c_void,
                mem::size_of_val(&size) as nix::libc::socklen_t,
            )
        };
        assert_eq!(rc, 0, "setsockopt(SO_RCVBUF) should succeed");
    }

    #[cfg(not(unix))]
    fn set_small_tcp_recv_buffer(_stream: &tokio::net::TcpStream) {}

    async fn spawn_tcp_one_shot_sender(payload: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(&payload).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        endpoint
    }

    fn json_frame(frame_type: FrameType, stream_id: u32, meta: Value) -> Frame {
        Frame {
            frame_type,
            flags: 0,
            stream_id,
            meta: serde_json::to_vec(&meta).unwrap(),
            data: Vec::new(),
        }
    }

    fn data_frame(frame_type: FrameType, stream_id: u32, data: Vec<u8>) -> Frame {
        Frame {
            frame_type,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data,
        }
    }

    fn endpoint_from_frame(frame: &Frame) -> String {
        serde_json::from_slice::<Value>(&frame.meta).unwrap()["endpoint"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn assert_limit_error(frame: Frame, stream_id: u32) {
        assert_error_code(frame, stream_id, "port_tunnel_limit_exceeded");
    }

    fn assert_error_code(frame: Frame, stream_id: u32, code: &str) {
        assert_eq!(frame.frame_type, FrameType::Error);
        assert_eq!(frame.stream_id, stream_id);
        let meta = serde_json::from_slice::<Value>(&frame.meta).unwrap();
        assert_eq!(meta["code"], code);
        assert_eq!(meta["fatal"], false);
    }

    fn sorted_payloads<const N: usize>(payloads: [Vec<u8>; N]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::from(payloads);
        payloads.sort();
        payloads
    }
}
