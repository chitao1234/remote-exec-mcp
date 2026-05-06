use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, read_preface, write_frame};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{AppState, HostRpcError};

const READ_BUF_SIZE: usize = 64 * 1024;

struct TunnelState {
    target: String,
    cancel: CancellationToken,
    tx: mpsc::Sender<Frame>,
    tcp_writers: Mutex<HashMap<u32, Arc<Mutex<OwnedWriteHalf>>>>,
    udp_sockets: Mutex<HashMap<u32, Arc<UdpSocket>>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
    next_daemon_stream_id: AtomicU32,
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

pub async fn serve_tunnel<S>(state: Arc<AppState>, stream: S) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    read_preface(&mut reader)
        .await
        .map_err(|err| rpc_error("invalid_port_tunnel", err.to_string()))?;

    let (tx, mut rx) = mpsc::channel::<Frame>(128);
    let tunnel = Arc::new(TunnelState {
        target: state.config.target.clone(),
        cancel: state.shutdown.child_token(),
        tx: tx.clone(),
        tcp_writers: Mutex::new(HashMap::new()),
        udp_sockets: Mutex::new(HashMap::new()),
        stream_cancels: Mutex::new(HashMap::new()),
        next_daemon_stream_id: AtomicU32::new(2),
    });
    let writer_cancel = tunnel.cancel.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_cancel.cancelled() => return,
                frame = rx.recv() => {
                    let Some(frame) = frame else {
                        return;
                    };
                    if write_frame(&mut writer, &frame).await.is_err() {
                        writer_cancel.cancel();
                        return;
                    }
                }
            }
        }
    });

    let result = tunnel_read_loop(tunnel.clone(), &mut reader).await;
    tunnel.cancel.cancel();
    drop(tx);
    writer_task.abort();
    result
}

async fn tunnel_read_loop<R>(tunnel: Arc<TunnelState>, reader: &mut R) -> Result<(), HostRpcError>
where
    R: AsyncRead + Unpin,
{
    loop {
        let frame = tokio::select! {
            _ = tunnel.cancel.cancelled() => return Ok(()),
            frame = read_frame(reader) => {
                match frame {
                    Ok(frame) => frame,
                    Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(()),
                    Err(err) => {
                        let _ = send_tunnel_error(&tunnel, 0, "invalid_port_tunnel", err.to_string(), true)
                            .await;
                        return Err(rpc_error("invalid_port_tunnel", err.to_string()));
                    }
                }
            }
        };

        let stream_id = frame.stream_id;
        if let Err(err) = handle_tunnel_frame(tunnel.clone(), frame).await {
            let _ =
                send_tunnel_error(&tunnel, stream_id, err.code, err.message.clone(), false).await;
        }
    }
}

async fn handle_tunnel_frame(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    match frame.frame_type {
        FrameType::TcpListen => tunnel_tcp_listen(tunnel, frame).await,
        FrameType::TcpConnect => tunnel_tcp_connect(tunnel, frame).await,
        FrameType::TcpData => tunnel_tcp_data(&tunnel, frame.stream_id, &frame.data).await,
        FrameType::TcpEof => tunnel_tcp_eof(&tunnel, frame.stream_id).await,
        FrameType::Close => tunnel_close_stream(&tunnel, frame.stream_id).await,
        FrameType::UdpBind => tunnel_udp_bind(tunnel, frame).await,
        FrameType::UdpDatagram => tunnel_udp_datagram(&tunnel, frame).await,
        _ => Err(rpc_error(
            "invalid_port_tunnel",
            format!("unexpected frame type `{:?}` from broker", frame.frame_type),
        )),
    }
}

async fn tunnel_tcp_listen(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let listener = TcpListener::bind(&endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpListenOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await?;

    tracing::info!(
        target = %tunnel.target,
        stream_id = frame.stream_id,
        endpoint = %bound_endpoint,
        "opened port tunnel tcp listener"
    );
    tokio::spawn(tunnel_tcp_accept_loop(
        tunnel,
        frame.stream_id,
        listener,
        stream_cancel,
    ));
    Ok(())
}

async fn tunnel_tcp_accept_loop(
    tunnel: Arc<TunnelState>,
    listener_stream_id: u32,
    listener: TcpListener,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    listener_stream_id,
                    "port_accept_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let stream_id = tunnel.next_daemon_stream_id.fetch_add(2, Ordering::Relaxed);
        let (reader, writer) = stream.into_split();
        tunnel
            .tcp_writers
            .lock()
            .await
            .insert(stream_id, Arc::new(Mutex::new(writer)));
        let stream_cancel = tunnel.cancel.child_token();
        tunnel
            .stream_cancels
            .lock()
            .await
            .insert(stream_id, stream_cancel.clone());
        if tunnel
            .send(Frame {
                frame_type: FrameType::TcpAccept,
                flags: 0,
                stream_id,
                meta: match encode_frame_meta(&TcpAcceptMeta {
                    listener_stream_id,
                    peer: peer.to_string(),
                }) {
                    Ok(meta) => meta,
                    Err(err) => {
                        let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false)
                            .await;
                        continue;
                    }
                },
                data: Vec::new(),
            })
            .await
            .is_err()
        {
            return;
        }
        tokio::spawn(tunnel_tcp_read_loop(
            tunnel.clone(),
            stream_id,
            reader,
            stream_cancel,
        ));
    }
}

async fn tunnel_tcp_connect(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let stream = TcpStream::connect(endpoint.as_str())
        .await
        .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?;
    let (reader, writer) = stream.into_split();
    tunnel
        .tcp_writers
        .lock()
        .await
        .insert(frame.stream_id, Arc::new(Mutex::new(writer)));
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpConnectOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_tcp_read_loop(
        tunnel,
        frame.stream_id,
        reader,
        stream_cancel,
    ));
    Ok(())
}

async fn tunnel_tcp_read_loop(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => return,
            read = reader.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ = tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpEof,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                return;
            }
            Ok(read) => {
                if tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: buf[..read].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        }
    }
}

async fn tunnel_tcp_data(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
    data: &[u8],
) -> Result<(), HostRpcError> {
    let writer = tunnel
        .tcp_writers
        .lock()
        .await
        .get(&stream_id)
        .cloned()
        .ok_or_else(|| {
            rpc_error(
                "unknown_port_connection",
                format!("unknown tunnel tcp stream `{stream_id}`"),
            )
        })?;
    writer
        .lock()
        .await
        .write_all(data)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))
}

async fn tunnel_tcp_eof(tunnel: &Arc<TunnelState>, stream_id: u32) -> Result<(), HostRpcError> {
    if let Some(writer) = tunnel.tcp_writers.lock().await.get(&stream_id).cloned() {
        writer
            .lock()
            .await
            .shutdown()
            .await
            .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    }
    Ok(())
}

async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    if let Some(cancel) = tunnel.stream_cancels.lock().await.remove(&stream_id) {
        cancel.cancel();
    }
    tunnel.tcp_writers.lock().await.remove(&stream_id);
    tunnel.udp_sockets.lock().await.remove(&stream_id);
    Ok(())
}

async fn tunnel_udp_bind(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let socket = Arc::new(
        UdpSocket::bind(&endpoint)
            .await
            .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?,
    );
    let bound_endpoint = socket
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    tunnel
        .udp_sockets
        .lock()
        .await
        .insert(frame.stream_id, socket.clone());
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::UdpBindOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint,
            })?,
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_udp_read_loop(
        tunnel,
        frame.stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}

async fn tunnel_udp_read_loop(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => return,
            received = socket.recv_from(&mut buf) => received,
        };
        let (read, peer) = match received {
            Ok(received) => received,
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let meta = match encode_frame_meta(&UdpDatagramMeta {
            peer: peer.to_string(),
        }) {
            Ok(meta) => meta,
            Err(err) => {
                let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false).await;
                return;
            }
        };
        if tunnel
            .send(Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id,
                meta,
                data: buf[..read].to_vec(),
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn tunnel_udp_datagram(tunnel: &Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: UdpDatagramMeta = decode_frame_meta(&frame)?;
    let socket = tunnel
        .udp_sockets
        .lock()
        .await
        .get(&frame.stream_id)
        .cloned()
        .ok_or_else(|| {
            rpc_error(
                "unknown_port_bind",
                format!("unknown tunnel udp stream `{}`", frame.stream_id),
            )
        })?;
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    Ok(())
}

impl TunnelState {
    async fn send(&self, frame: Frame) -> Result<(), HostRpcError> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| rpc_error("port_tunnel_closed", "port tunnel writer is closed"))
    }
}

async fn send_tunnel_error(
    tunnel: &TunnelState,
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    let meta = encode_frame_meta(&ErrorMeta {
        code: code.into(),
        message: message.into(),
        fatal,
    })?;
    tunnel
        .send(Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id,
            meta,
            data: Vec::new(),
        })
        .await
}

fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, HostRpcError> {
    serde_json::from_slice(&frame.meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, HostRpcError> {
    serde_json::to_vec(meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    HostRpcError {
        status: 400,
        code,
        message,
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
                experimental_apply_patch_target_encoding_autodetect: false,
                process_environment: ProcessEnvironment::capture_current(),
            })
            .unwrap(),
        )
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
