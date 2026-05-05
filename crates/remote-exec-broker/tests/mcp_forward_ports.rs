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
        let mut buf = vec![0u8; 64];
        let read = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..read]).await.unwrap();
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
    stream.write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello");

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
        let (len, peer) = echo_socket.recv_from(&mut buf).await.unwrap();
        echo_socket.send_to(&buf[..len], peer).await.unwrap();
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

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client
        .send_to(b"hello-udp", &listen_endpoint)
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let (read, _) = client.recv_from(&mut buf).await.unwrap();
    assert_eq!(&buf[..read], b"hello-udp");

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
