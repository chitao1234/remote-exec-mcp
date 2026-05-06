mod support;

use std::net::SocketAddr;
use std::time::Duration;

use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN,
    read_frame, write_frame, write_preface,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn port_tunnel_upgrade_accepts_http11_and_binds_tcp_listener() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
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
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
async fn dropping_port_tunnel_releases_tcp_listener_when_tunnel_closes() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
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

    drop(stream);
    wait_until_bindable(&endpoint).await;
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
