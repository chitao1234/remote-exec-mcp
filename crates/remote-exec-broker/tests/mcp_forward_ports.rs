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
async fn forward_ports_keeps_forward_open_after_stream_connect_error() {
    let fixture = support::spawners::spawn_broker_local_only().await;
    let destination_probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination_addr = destination_probe.local_addr().unwrap();
    drop(destination_probe);

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": destination_addr.to_string(),
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

    tokio::time::sleep(Duration::from_millis(250)).await;
    let listed = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "list",
                "forward_ids": [forward_id.clone()]
            }),
        )
        .await;
    assert_eq!(listed.structured_content["forwards"][0]["status"], "open");
    assert_eq!(
        listed.structured_content["forwards"][0]
            .get("last_error")
            .and_then(|value| value.as_str()),
        None
    );

    let echo_listener = tokio::net::TcpListener::bind(destination_addr)
        .await
        .unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = echo_listener.accept().await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        stream.write_all(&buf).await.unwrap();
    });

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"after-error").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, b"after-error");

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

    let open = fixture
        .open_remote_tcp_forward(&echo_addr.to_string())
        .await;
    let forward_id = forward_id_from(&open);
    let listen_endpoint = listen_endpoint_from(&open);

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state).await;

    let forward =
        wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
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
async fn forward_ports_closes_active_tcp_streams_after_connect_tunnel_drop() {
    let fixture = support::spawners::spawn_broker_with_local_and_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match echo_listener.accept().await {
                Ok(value) => value,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    let read = match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    if stream.write_all(&buf[..read]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "builder-a",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = forward_id_from(&open);
    let listen_endpoint = listen_endpoint_from(&open);

    let mut active = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    active.write_all(b"before").await.unwrap();
    let mut echoed = [0u8; 6];
    active.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"before");

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state).await;
    let read_after_drop = tokio::time::timeout(Duration::from_secs(5), async {
        let mut buf = [0u8; 1];
        active.read(&mut buf).await
    })
    .await
    .expect("active tcp stream should close when connect-side tunnel drops");
    match read_after_drop {
        Ok(0) => {}
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Ok(read) => panic!("expected active tcp stream to close, read {read} byte(s) instead"),
        Err(err) => panic!("unexpected active tcp stream read error: {err}"),
    }

    let forward =
        wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(forward["status"], "open");

    let mut later = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    later.write_all(b"after").await.unwrap();
    let mut echoed_later = [0u8; 5];
    later.read_exact(&mut echoed_later).await.unwrap();
    assert_eq!(&echoed_later, b"after");

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

#[tokio::test]
async fn forward_ports_close_reports_listen_cleanup_failures() {
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

    let error = fixture
        .call_tool_error(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id]
            }),
        )
        .await;

    assert!(
        error.contains("closing port forward")
            && (error.contains("port tunnel closed")
                || error.contains("resuming port tunnel session")
                || error.contains("waiting to resume port tunnel session")),
        "unexpected close error: {error}"
    );
}

#[tokio::test]
async fn forward_ports_records_failure_when_runtime_fails_before_multi_open_finishes() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    support::stub_daemon::fail_first_forward_runtime_before_multi_open_finishes(
        &fixture.stub_state,
    )
    .await;
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blackhole_addr = blackhole.local_addr().unwrap();
    drop(blackhole);

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [
                    {
                        "listen_endpoint": "127.0.0.1:0",
                        "connect_endpoint": blackhole_addr.to_string(),
                        "protocol": "tcp"
                    },
                    {
                        "listen_endpoint": "127.0.0.1:0",
                        "connect_endpoint": blackhole_addr.to_string(),
                        "protocol": "tcp"
                    }
                ]
            }),
        )
        .await;
    let first_forward_id = forward_id_at(&open, 0);

    let failed = wait_for_forward_status(
        &fixture,
        &first_forward_id,
        "failed",
        Duration::from_secs(5),
    )
    .await;
    let error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("forced_resume_failure")
            || error.contains("forced resume failure")
            || error.contains("port tunnel closed"),
        "unexpected failure error: {error}"
    );
}

#[tokio::test]
async fn forward_ports_closes_pending_tcp_stream_when_remote_connect_never_acknowledges() {
    const MAX_PENDING_TCP_BYTES_PER_STREAM: usize = 256 * 1024;

    let fixture = support::spawners::spawn_broker_with_local_and_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    support::stub_daemon::drop_tcp_connect_ok_frames(&fixture.stub_state).await;

    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return,
                Ok(Ok(_)) => {}
            }
        }
    });

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "builder-a",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": upstream_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = forward_id_from(&open);
    let listen_endpoint = listen_endpoint_from(&open);

    let mut client = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    let oversized = vec![7u8; MAX_PENDING_TCP_BYTES_PER_STREAM + 64 * 1024];
    let _ = client.write_all(&oversized).await;

    let close_result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut buf = [0u8; 1];
        client.read(&mut buf).await
    })
    .await
    .expect("pending tcp stream should close once the buffer limit is exceeded");
    match close_result {
        Ok(0) => {}
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Ok(read) => panic!("expected pending tcp stream to close, read {read} byte(s) instead"),
        Err(err) => panic!("unexpected pending tcp stream read error: {err}"),
    }

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
async fn forward_ports_stays_open_during_heavy_local_udp_peer_churn() {
    const UDP_PEER_COUNT: usize = 320;

    let fixture = support::spawners::spawn_broker_local_only().await;
    let echo_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        loop {
            let (read, peer) = match echo_socket.recv_from(&mut buf).await {
                Ok(value) => value,
                Err(_) => return,
            };
            if echo_socket.send_to(&buf[..read], peer).await.is_err() {
                return;
            }
        }
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
    let forward_id = forward_id_from(&open);
    let listen_endpoint = listen_endpoint_from(&open);

    let mut peers = Vec::with_capacity(UDP_PEER_COUNT);
    for _ in 0..UDP_PEER_COUNT {
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer.send_to(b"peer", &listen_endpoint).await.unwrap();
        peers.push(peer);
    }

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
    forward_id_at(result, 0)
}

fn forward_id_at(result: &support::fixture::ToolResult, index: usize) -> String {
    result.structured_content["forwards"][index]["forward_id"]
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
