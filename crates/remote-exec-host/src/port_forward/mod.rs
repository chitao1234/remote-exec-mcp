mod codec;
mod error;
mod session;
mod session_store;
mod tcp;
mod tunnel;
mod udp;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use remote_exec_proto::port_tunnel::Frame;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::AppState;

pub use session_store::TunnelSessionStore;
pub use tunnel::serve_tunnel;

const READ_BUF_SIZE: usize = 64 * 1024;
#[cfg(not(test))]
const RESUME_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const RESUME_TIMEOUT: Duration = Duration::from_millis(100);

struct TunnelState {
    state: Arc<AppState>,
    cancel: CancellationToken,
    tx: mpsc::Sender<Frame>,
    tcp_writers: Mutex<HashMap<u32, Arc<Mutex<OwnedWriteHalf>>>>,
    udp_sockets: Mutex<HashMap<u32, Arc<UdpSocket>>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
    next_daemon_stream_id: AtomicU32,
    attached_session: Mutex<Option<Arc<session::SessionState>>>,
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
        self.tx
            .send(frame)
            .await
            .map_err(|_| error::rpc_error("port_tunnel_closed", "port tunnel writer is closed"))
    }
}

#[cfg(test)]
mod port_tunnel_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, read_frame, write_frame, write_preface,
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

    fn test_state() -> Arc<AppState> {
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
                port_forward_limits: crate::HostPortForwardLimits::default(),
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

    fn sorted_payloads<const N: usize>(payloads: [Vec<u8>; N]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::from(payloads);
        payloads.sort();
        payloads
    }
}
