mod support;

use std::net::SocketAddr;
use std::time::Duration;

use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, TunnelCloseMeta,
    TunnelErrorMeta, TunnelForwardProtocol, TunnelOpenMeta, TunnelReadyMeta, TunnelRole,
    UPGRADE_TOKEN, read_frame, write_frame, write_preface,
};
use support::test_helpers::DEFAULT_TEST_TARGET;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn port_tunnel_upgrade_accepts_http11_and_binds_tcp_listener() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    open_listen_tunnel(&mut stream, "fwd_bind").await;
    write_frame(
        &mut stream,
        &json_frame(
            FrameType::TcpListen,
            1,
            serde_json::json!({ "endpoint": "127.0.0.1:0" }),
        ),
    )
    .await
    .unwrap();

    let ok = read_frame(&mut stream).await.unwrap();
    assert_eq!(ok.frame_type, FrameType::TcpListenOk);
    let endpoint = endpoint_from_frame(&ok);
    let _probe = tokio::net::TcpStream::connect(endpoint).await.unwrap();
}

#[tokio::test]
async fn port_tunnel_rejects_http10_upgrade_request() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let response = raw_http_request(
        fixture.addr,
        &format!(
            "POST /v1/port/tunnel HTTP/1.0\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: {UPGRADE_TOKEN}\r\n{TUNNEL_PROTOCOL_VERSION_HEADER}: {TUNNEL_PROTOCOL_VERSION}\r\nContent-Length: 0\r\n\r\n"
        ),
    )
    .await;

    if !response.is_empty() {
        assert!(
            response.starts_with("HTTP/1.0 400 Bad Request\r\n")
                || response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{response}"
        );
    }
}

#[tokio::test]
async fn port_tunnel_requires_v4_header() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let response = fixture
        .client
        .post(fixture.url("/v1/port/tunnel"))
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, UPGRADE_TOKEN)
        .header(TUNNEL_PROTOCOL_VERSION_HEADER, "3")
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: remote_exec_proto::rpc::RpcErrorBody = response.json().await.unwrap();
    assert_eq!(body.wire_code(), "bad_request");
    assert!(
        body.message
            .contains("x-remote-exec-port-tunnel-version: 4")
    );
}

#[tokio::test]
async fn port_tunnel_rejects_reserved_legacy_session_frames() {
    for frame_type in [FrameType::SessionOpen, FrameType::SessionResume] {
        let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
        let mut stream = open_tunnel(fixture.addr).await;

        write_preface(&mut stream).await.unwrap();
        write_frame(
            &mut stream,
            &json_frame(frame_type, 0, serde_json::json!({ "session_id": "legacy" })),
        )
        .await
        .unwrap();

        let error = read_frame(&mut stream).await.unwrap();
        assert_eq!(error.frame_type, FrameType::Error);
        assert_eq!(error.stream_id, 0);
        let error_meta: TunnelErrorMeta = serde_json::from_slice(&error.meta).unwrap();
        assert_eq!(error_meta.wire_code(), "invalid_port_tunnel");
        assert!(
            error_meta.message.contains("unexpected frame type"),
            "unexpected legacy-frame error message: {}",
            error_meta.message
        );
    }
}

#[tokio::test]
async fn tunnel_open_ready_and_close_round_trip() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelOpenMeta {
                forward_id: ForwardId::new("fwd_test"),
                role: TunnelRole::Listen,
                side: DEFAULT_TEST_TARGET.to_string(),
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

    let ready = read_frame(&mut stream).await.unwrap();
    assert_eq!(ready.frame_type, FrameType::TunnelReady);
    let ready_meta: TunnelReadyMeta = serde_json::from_slice(&ready.meta).unwrap();
    assert_eq!(ready_meta.generation, 1);
    assert!(ready_meta.session_id.is_some());

    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelCloseMeta::operator_close("fwd_test", 1)).unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    let closed = read_frame(&mut stream).await.unwrap();
    assert_eq!(closed.frame_type, FrameType::TunnelClosed);
}

#[tokio::test]
async fn tunnel_open_ready_reports_configured_limits() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        DEFAULT_TEST_TARGET,
        r#"[port_forward_limits]
max_active_tcp_streams = 3
max_udp_binds = 5
max_tunnel_queued_bytes = 4096
"#,
    )
    .await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelOpenMeta {
                forward_id: ForwardId::new("fwd_limits"),
                role: TunnelRole::Connect,
                side: DEFAULT_TEST_TARGET.to_string(),
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

    let ready = read_frame(&mut stream).await.unwrap();
    assert_eq!(ready.frame_type, FrameType::TunnelReady);
    let ready_meta: TunnelReadyMeta = serde_json::from_slice(&ready.meta).unwrap();
    assert_eq!(ready_meta.generation, 7);
    assert_eq!(ready_meta.limits.max_active_tcp_streams, 3);
    assert_eq!(ready_meta.limits.max_udp_peers, 5);
    assert_eq!(ready_meta.limits.max_queued_bytes, 4096);
}

#[tokio::test]
async fn port_tunnel_rejects_retained_session_limit() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        DEFAULT_TEST_TARGET,
        r#"[port_forward_limits]
max_retained_sessions = 1
"#,
    )
    .await;

    let mut first = open_tunnel(fixture.addr).await;
    write_preface(&mut first).await.unwrap();
    open_listen_tunnel(&mut first, "fwd_first").await;

    let mut second = open_tunnel(fixture.addr).await;
    write_preface(&mut second).await.unwrap();
    write_tunnel_open(&mut second, "fwd_second").await;

    let error = read_frame(&mut second).await.unwrap();
    assert_eq!(error.frame_type, FrameType::Error);
    assert_eq!(error.stream_id, 0);
    let error_meta: TunnelErrorMeta = serde_json::from_slice(&error.meta).unwrap();
    assert_eq!(error_meta.wire_code(), "port_tunnel_limit_exceeded");
}

#[tokio::test]
async fn port_tunnel_rejects_second_concurrent_tunnel_limit() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        DEFAULT_TEST_TARGET,
        r#"[port_forward_limits]
max_tunnel_connections = 1
"#,
    )
    .await;

    let mut first = open_tunnel(fixture.addr).await;
    write_preface(&mut first).await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(1), rejected_tunnel(fixture.addr))
        .await
        .expect("second tunnel should be rejected promptly");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let error: remote_exec_proto::rpc::RpcErrorBody = response.json().await.unwrap();
    assert_eq!(error.wire_code(), "port_tunnel_limit_exceeded");
}

#[tokio::test]
async fn port_tunnel_rejects_old_generation_frames() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelOpenMeta {
                forward_id: ForwardId::new("fwd_test"),
                role: TunnelRole::Listen,
                side: DEFAULT_TEST_TARGET.to_string(),
                generation: 2,
                protocol: TunnelForwardProtocol::Tcp,
                resume_session_id: None,
            })
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();
    let ready = read_frame(&mut stream).await.unwrap();
    assert_eq!(ready.frame_type, FrameType::TunnelReady);

    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelCloseMeta::from_raw_reason(
                "fwd_test",
                1,
                "stale_close",
            ))
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    let error = read_frame(&mut stream).await.unwrap();
    assert_eq!(error.frame_type, FrameType::Error);
    assert_eq!(error.stream_id, 0);
    let error_meta: TunnelErrorMeta = serde_json::from_slice(&error.meta).unwrap();
    assert_eq!(error_meta.wire_code(), "port_tunnel_generation_mismatch");
    assert_eq!(error_meta.generation, Some(2));
}

#[tokio::test]
async fn tunnel_close_releases_tcp_listener() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    open_listen_tunnel(&mut stream, "fwd_close_release").await;
    write_frame(
        &mut stream,
        &json_frame(
            FrameType::TcpListen,
            1,
            serde_json::json!({ "endpoint": "127.0.0.1:0" }),
        ),
    )
    .await
    .unwrap();
    let ok = read_frame(&mut stream).await.unwrap();
    let endpoint = endpoint_from_frame(&ok);

    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelCloseMeta::operator_close("fwd_close_release", 1))
                .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();
    let closed = read_frame(&mut stream).await.unwrap();
    assert_eq!(closed.frame_type, FrameType::TunnelClosed);

    drop(stream);
    wait_until_bindable(&endpoint).await;
}

#[tokio::test]
async fn terminal_port_tunnel_error_releases_tcp_listener_without_waiting_for_resume_timeout() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    open_listen_tunnel(&mut stream, "fwd_terminal_release").await;

    write_frame(
        &mut stream,
        &json_frame(
            FrameType::TcpListen,
            1,
            serde_json::json!({ "endpoint": "127.0.0.1:0" }),
        ),
    )
    .await
    .unwrap();
    let ok = read_frame(&mut stream).await.unwrap();
    let endpoint = endpoint_from_frame(&ok);

    stream
        .write_all(&[
            FrameType::TcpData as u8,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ])
        .await
        .unwrap();

    wait_until_bindable(&endpoint).await;
}

#[tokio::test]
async fn terminal_port_tunnel_error_returns_fatal_error_frame_before_closing() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    open_listen_tunnel(&mut stream, "fwd_terminal_error").await;

    write_frame(
        &mut stream,
        &json_frame(
            FrameType::TcpListen,
            1,
            serde_json::json!({ "endpoint": "127.0.0.1:0" }),
        ),
    )
    .await
    .unwrap();
    let ok = read_frame(&mut stream).await.unwrap();
    let endpoint = endpoint_from_frame(&ok);

    stream
        .write_all(&[
            FrameType::TcpData as u8,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ])
        .await
        .unwrap();

    let error = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut stream))
        .await
        .expect("daemon should return an error frame before closing")
        .expect("daemon should return a valid error frame");
    assert_eq!(error.frame_type, FrameType::Error);
    assert_eq!(error.stream_id, 0);

    let meta = serde_json::from_slice::<serde_json::Value>(&error.meta).unwrap();
    assert_eq!(meta["code"], "invalid_port_tunnel");
    assert_eq!(meta["fatal"], true);
    assert!(
        meta["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );

    wait_until_bindable(&endpoint).await;
}

async fn open_listen_tunnel(stream: &mut tokio::net::TcpStream, forward_id: &str) {
    write_tunnel_open(stream, forward_id).await;
    let ready = read_frame(stream).await.unwrap();
    assert_eq!(ready.frame_type, FrameType::TunnelReady);
}

async fn write_tunnel_open(stream: &mut tokio::net::TcpStream, forward_id: &str) {
    write_frame(
        stream,
        &Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelOpenMeta {
                forward_id: ForwardId::new(forward_id),
                role: TunnelRole::Listen,
                side: DEFAULT_TEST_TARGET.to_string(),
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
}

async fn open_tunnel(addr: SocketAddr) -> tokio::net::TcpStream {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /v1/port/tunnel HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: {UPGRADE_TOKEN}\r\n{TUNNEL_PROTOCOL_VERSION_HEADER}: {TUNNEL_PROTOCOL_VERSION}\r\nContent-Length: 0\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    let mut buf = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut buf).await.unwrap();
        assert_ne!(read, 0, "unexpected EOF before upgrade response");
        response.extend_from_slice(&buf[..read]);
    }
    let response = String::from_utf8(response).unwrap();
    assert!(
        response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "{response}"
    );
    assert!(
        response
            .to_ascii_lowercase()
            .contains("upgrade: remote-exec-port-tunnel"),
        "{response}"
    );
    stream
}

async fn rejected_tunnel(addr: SocketAddr) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/port/tunnel"))
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, UPGRADE_TOKEN)
        .header(TUNNEL_PROTOCOL_VERSION_HEADER, TUNNEL_PROTOCOL_VERSION)
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .unwrap()
}

async fn raw_http_request(addr: SocketAddr, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}

fn json_frame(frame_type: FrameType, stream_id: u32, meta: serde_json::Value) -> Frame {
    Frame {
        frame_type,
        flags: 0,
        stream_id,
        meta: serde_json::to_vec(&meta).unwrap(),
        data: Vec::new(),
    }
}

fn endpoint_from_frame(frame: &Frame) -> String {
    serde_json::from_slice::<serde_json::Value>(&frame.meta).unwrap()["endpoint"]
        .as_str()
        .unwrap()
        .to_string()
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
