mod codec;
mod error;
mod limiter;
mod session;
mod session_store;
mod tcp;
mod tunnel;
mod udp;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::time::Duration;

use remote_exec_proto::port_tunnel::{Frame, HEADER_LEN};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::AppState;

pub use session_store::TunnelSessionStore;
pub use tunnel::serve_tunnel;

pub use limiter::PortForwardLimiter;

const READ_BUF_SIZE: usize = 64 * 1024;
#[cfg(not(test))]
const RESUME_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const RESUME_TIMEOUT: Duration = Duration::from_millis(100);

struct TunnelState {
    state: Arc<AppState>,
    cancel: CancellationToken,
    tx: TunnelSender,
    tcp_writers: Mutex<HashMap<u32, Arc<Mutex<OwnedWriteHalf>>>>,
    tcp_stream_permits: Mutex<HashMap<u32, limiter::PortForwardPermit>>,
    udp_sockets: Mutex<HashMap<u32, TransportUdpBind>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
    next_daemon_stream_id: AtomicU32,
    generation: AtomicU64,
    attached_session: Mutex<Option<Arc<session::SessionState>>>,
    _connection_permit: limiter::PortForwardPermit,
}

#[derive(Clone)]
struct TunnelSender {
    tx: mpsc::Sender<QueuedFrame>,
    limiter: Arc<PortForwardLimiter>,
}

struct QueuedFrame {
    frame: Frame,
    _permit: Option<limiter::PortForwardPermit>,
}

struct TransportUdpBind {
    socket: Arc<UdpSocket>,
    _permit: limiter::PortForwardPermit,
}

#[derive(Debug, Deserialize)]
struct EndpointMeta {
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct EndpointOkMeta {
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct TcpAcceptMeta {
    listener_stream_id: u32,
    peer: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UdpDatagramMeta {
    peer: String,
}

#[derive(Debug, Serialize)]
struct ErrorMeta {
    code: String,
    message: String,
    fatal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SessionResumeMeta {
    session_id: String,
}

#[derive(Debug, Serialize)]
struct SessionReadyMeta {
    session_id: String,
    resume_timeout_ms: u64,
}

enum TunnelMode {
    Transport,
    Session(Arc<session::SessionState>),
}

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
        self.tx
            .send(queued)
            .await
            .map_err(|_| error::rpc_error("port_tunnel_closed", "port tunnel writer is closed"))
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

#[cfg(test)]
mod port_tunnel_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, TunnelForwardProtocol, TunnelOpenMeta, TunnelReadyMeta, TunnelRole,
        read_frame, write_frame, write_preface,
    };
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    use super::*;
    use crate::{
        HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig, build_runtime_state,
    };

    #[tokio::test]
    async fn tunnel_binds_tcp_listener_and_releases_it_on_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state.clone(), daemon_side));

        write_preface(&mut broker_side).await.unwrap();
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
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);

        wait_until_bindable(&bound_endpoint).await;
    }

    #[tokio::test]
    async fn tunnel_tcp_connect_echoes_binary_data_full_duplex() {
        let state = test_state();
        let echo_endpoint = spawn_tcp_echo_server().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
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
    async fn tunnel_udp_bind_relays_datagrams_from_two_peers() {
        let state = test_state();
        let endpoint = free_loopback_endpoint().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
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
        let listen_endpoint = free_loopback_endpoint().await;
        let (listen_bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id).await;
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
        let listen_endpoint = free_loopback_endpoint().await;
        let (_listen_bound_endpoint, session_id) =
            open_v4_resumable_tcp_listener(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id).await;
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
        let listen_endpoint = free_loopback_endpoint().await;
        let (listen_bound_endpoint, session_id) =
            open_resumable_udp_bind(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id).await;
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
        let listen_endpoint = free_loopback_endpoint().await;
        let (bound_endpoint, _session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        tokio::time::sleep(Duration::from_millis(250)).await;

        wait_until_bindable(&bound_endpoint).await;
    }

    #[tokio::test]
    async fn detached_tcp_listener_accepts_after_resume_not_before() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        let client = tokio::net::TcpStream::connect(&bound_endpoint)
            .await
            .unwrap();
        let mut resumed = resume_session(&state, &session_id).await;

        let frame = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut resumed))
            .await
            .expect("accept should arrive after resume")
            .unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpAccept);

        drop(client);
    }

    #[tokio::test]
    async fn tunnel_reports_tcp_listen_errors_on_request_stream() {
        let state = test_state();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_endpoint = occupied.local_addr().unwrap().to_string();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
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
    async fn retained_tcp_listener_limit_rejects_second_session_listener() {
        let state = test_state_with_limits(crate::HostPortForwardLimits {
            max_retained_sessions: 2,
            max_retained_listeners: 1,
            ..crate::HostPortForwardLimits::default()
        });
        let first_endpoint = free_loopback_endpoint().await;
        let second_endpoint = free_loopback_endpoint().await;
        let (_first_bound, _first_session_id) =
            open_resumable_tcp_listener(&state, &first_endpoint).await;

        let mut second = start_tunnel(state).await;
        write_frame(
            &mut second,
            &json_frame(FrameType::SessionOpen, 0, serde_json::json!({})),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut second).await.unwrap().frame_type,
            FrameType::SessionReady
        );
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
        let mut broker_side = start_tunnel(state).await;

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
        let mut broker_side = start_tunnel(state).await;

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
        let mut broker_side = start_tunnel(state).await;

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
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

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

        let no_listener_error =
            tokio::time::timeout(Duration::from_millis(100), read_frame(&mut broker_side)).await;
        assert!(
            no_listener_error.is_err(),
            "listener stream should not receive recoverable pressure errors"
        );
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
        let mut broker_side = start_tunnel(state).await;

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

    async fn open_resumable_tcp_listener(
        state: &Arc<AppState>,
        endpoint: &str,
    ) -> (String, String) {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &Frame {
                frame_type: FrameType::SessionOpen,
                flags: 0,
                stream_id: 0,
                meta: Vec::new(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        let ready = read_frame(&mut broker_side).await.unwrap();
        let session_id = serde_json::from_slice::<Value>(&ready.meta).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
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
        let session_id = serde_json::from_slice::<Value>(&ready.meta).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
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

    async fn open_resumable_udp_bind(state: &Arc<AppState>, endpoint: &str) -> (String, String) {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &Frame {
                frame_type: FrameType::SessionOpen,
                flags: 0,
                stream_id: 0,
                meta: Vec::new(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        let ready = read_frame(&mut broker_side).await.unwrap();
        let session_id = serde_json::from_slice::<Value>(&ready.meta).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
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

    async fn resume_session(state: &Arc<AppState>, session_id: &str) -> DuplexStream {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::SessionResume,
                0,
                serde_json::json!({ "session_id": session_id }),
            ),
        )
        .await
        .unwrap();
        let resumed = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(resumed.frame_type, FrameType::SessionResumed);
        broker_side
    }

    async fn free_loopback_endpoint() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        drop(listener);
        endpoint
    }

    async fn wait_until_bindable(endpoint: &str) {
        for _ in 0..40 {
            if tokio::net::TcpListener::bind(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("endpoint `{endpoint}` did not become bindable");
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
        assert_eq!(frame.frame_type, FrameType::Error);
        assert_eq!(frame.stream_id, stream_id);
        let meta = serde_json::from_slice::<Value>(&frame.meta).unwrap();
        assert_eq!(meta["code"], "port_tunnel_limit_exceeded");
        assert_eq!(meta["fatal"], false);
    }

    fn sorted_payloads<const N: usize>(payloads: [Vec<u8>; N]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::from(payloads);
        payloads.sort();
        payloads
    }
}
