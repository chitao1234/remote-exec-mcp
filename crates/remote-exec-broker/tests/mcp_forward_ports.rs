#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn forward_ports_is_listed_for_mcp_clients() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let tools = fixture
        .client
        .list_tools(Some(rmcp::model::PaginatedRequestParams::default()))
        .await
        .expect("list tools");

    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == "forward_ports")
    );
}

#[tokio::test]
async fn forward_ports_opens_lists_and_closes_local_tcp_forward() {
    let fixture = support::spawners::spawn_broker_local_only().await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = echo_listener.accept().await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        stream.write_all(&buf).await.unwrap();
    });

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(listen_endpoint, "127.0.0.1:0");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    let mut payload = Vec::with_capacity(96 * 1024 + 4);
    payload.extend_from_slice(&[0, 1, 2, 255]);
    payload.extend((0..96 * 1024).map(|idx| (idx % 251) as u8));
    stream.write_all(&payload).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, payload);

    let list = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "list",
                "forward_ids": [forward_id.clone()]
            }),
        )
        .await;
    assert_eq!(list.structured_content["forwards"][0]["status"], "open");

    let close = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id]
            }),
        )
        .await;
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn forward_ports_forwards_local_udp_datagrams() {
    let fixture = support::spawners::spawn_broker_local_only().await;
    let echo_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        let first = echo_socket.recv_from(&mut buf).await.unwrap();
        let first_payload = buf[..first.0].to_vec();
        let second = echo_socket.recv_from(&mut buf).await.unwrap();
        let second_payload = buf[..second.0].to_vec();
        echo_socket.send_to(&first_payload, first.1).await.unwrap();
        echo_socket
            .send_to(&second_payload, second.1)
            .await
            .unwrap();
    });

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "udp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    let client_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let client_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client_a
        .send_to(b"hello-udp-a", &listen_endpoint)
        .await
        .unwrap();
    client_b
        .send_to(b"hello-udp-b", &listen_endpoint)
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let read_a = tokio::time::timeout(Duration::from_secs(2), client_a.recv_from(&mut buf))
        .await
        .expect("client a should receive udp reply")
        .unwrap()
        .0;
    assert_eq!(&buf[..read_a], b"hello-udp-a");
    let read_b = tokio::time::timeout(Duration::from_secs(2), client_b.recv_from(&mut buf))
        .await
        .expect("client b should receive udp reply")
        .unwrap()
        .0;
    assert_eq!(&buf[..read_b], b"hello-udp-b");

    let close = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id]
            }),
        )
        .await;
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn forward_ports_marks_forward_failed_after_background_connect_error() {
    let fixture = support::spawners::spawn_broker_local_only().await;
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blackhole_addr = blackhole.local_addr().unwrap();
    drop(blackhole);

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": blackhole_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    let _client = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();

    let failed =
        wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(5)).await;
    assert_eq!(failed["status"], "failed");
    let error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("connecting tcp forward destination"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn forward_ports_rejects_targets_without_tunnel_protocol_version() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(1).await;

    let error = fixture
        .call_tool_error(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": "127.0.0.1:1",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;

    assert!(
        error.contains("does not support port forward protocol version 3"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn forward_ports_keeps_forward_open_after_listen_tunnel_drop() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = echo_listener.accept().await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        stream.write_all(&buf).await.unwrap();
    });

    let open = fixture.open_remote_tcp_forward(&echo_addr.to_string()).await;
    let forward_id = forward_id_from(&open);
    let listen_endpoint = listen_endpoint_from(&open);

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state).await;

    let forward = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(forward["status"], "open");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"after").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, b"after");
}

#[tokio::test]
async fn forward_ports_fails_when_resume_deadline_expires() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    support::stub_daemon::block_session_resume(&fixture.stub_state).await;
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blackhole_addr = blackhole.local_addr().unwrap();
    drop(blackhole);

    let open = fixture
        .open_remote_tcp_forward(&blackhole_addr.to_string())
        .await;
    let forward_id = forward_id_from(&open);

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state).await;

    let failed =
        wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(10)).await;
    let error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("reconnect timed out")
            || error.contains("resume expired")
            || error.contains("port tunnel closed"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn forward_ports_does_not_retry_stream_error_frames() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    support::stub_daemon::set_port_tunnel_resume_error(
        &fixture.stub_state,
        "port_bind_failed",
        "boom",
    )
    .await;
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blackhole_addr = blackhole.local_addr().unwrap();
    drop(blackhole);

    let open = fixture
        .open_remote_tcp_forward(&blackhole_addr.to_string())
        .await;
    let forward_id = forward_id_from(&open);
    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state).await;

    let failed =
        wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(10)).await;
    let error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("port_bind_failed") || error.contains("boom"),
        "unexpected error: {error}"
    );
}

async fn wait_for_forward_status(
    fixture: &support::fixture::BrokerFixture,
    forward_id: &str,
    status: &str,
    timeout: Duration,
) -> serde_json::Value {
    let started = std::time::Instant::now();
    let mut last_entry = None;
    while started.elapsed() < timeout {
        let list = fixture
            .call_tool(
                "forward_ports",
                serde_json::json!({
                    "action": "list",
                    "forward_ids": [forward_id]
                }),
            )
            .await;
        let entry = list.structured_content["forwards"][0].clone();
        last_entry = Some(entry.clone());
        if entry["status"] == status {
            return entry;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let last_status = last_entry
        .as_ref()
        .and_then(|entry| entry["status"].as_str())
        .unwrap_or("<missing>");
    let last_error = last_entry
        .as_ref()
        .and_then(|entry| entry["last_error"].as_str())
        .unwrap_or_default();
    panic!(
        "forward `{forward_id}` did not reach status `{status}` within {timeout:?}; last_status={last_status} last_error={last_error}"
    );
}

fn forward_id_from(result: &support::fixture::ToolResult) -> String {
    result.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn listen_endpoint_from(result: &support::fixture::ToolResult) -> String {
    result.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string()
}
