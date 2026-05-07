mod support;

use std::time::Duration;

use remote_exec_broker::client::{Connection, RemoteExecClient};
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortSpec, ForwardPortsInput};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn sessions_are_isolated_per_target() {
    let cluster = support::spawn_cluster().await;

    let started = cluster
        .broker
        .call_tool(
            "exec_command",
            support::long_running_tty_exec_input("builder-a"),
        )
        .await;

    let session_id = started.structured_content["session_id"].as_str().unwrap();
    let polled = cluster
        .broker
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-a",
                "chars": "",
                "yield_time_ms": 250
            }),
        )
        .await;
    assert_eq!(polled.structured_content["target"], "builder-a");

    let mismatch = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-b",
                "chars": ""
            }),
        )
        .await;
    assert!(mismatch.contains("does not belong"), "mismatch: {mismatch}");
}

#[tokio::test]
async fn patch_and_image_calls_only_touch_the_selected_target() {
    let cluster = support::spawn_cluster().await;
    support::write_png(&cluster.daemon_b.workdir.join("builder-b.png"), 12, 8).await;

    cluster
        .broker
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: marker.txt\n+builder-a\n*** End Patch\n"
            }),
        )
        .await;

    let image = cluster
        .broker
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "builder-b",
                "path": "builder-b.png",
                "detail": "original"
            }),
        )
        .await;

    assert!(cluster.daemon_a.workdir.join("marker.txt").exists());
    assert!(!cluster.daemon_b.workdir.join("marker.txt").exists());
    assert_eq!(image.structured_content["target"], "builder-b");
    assert_eq!(image.raw_content[0]["type"], "input_image");

    let wrong_target = cluster
        .broker
        .call_tool_error(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "builder-b.png",
                "detail": "original"
            }),
        )
        .await;
    assert!(
        wrong_target.contains("No such file")
            || wrong_target.contains("os error 2")
            || wrong_target.contains("internal_error"),
        "wrong target error: {wrong_target}"
    );
}

#[tokio::test]
async fn sessions_are_invalidated_after_daemon_restart() {
    let mut cluster = support::spawn_cluster().await;
    let started = cluster
        .broker
        .call_tool(
            "exec_command",
            support::long_running_tty_exec_input("builder-a"),
        )
        .await;
    let session_id = started.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    cluster.daemon_a.restart().await;

    let invalidated = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-a",
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;
    assert_eq!(
        invalidated,
        format!("write_stdin failed: Unknown process id {session_id}"),
        "restart invalidation error: {invalidated}"
    );

    let unknown = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": started.structured_content["session_id"],
                "target": "builder-a",
                "chars": ""
            }),
        )
        .await;
    assert_eq!(
        unknown,
        format!("write_stdin failed: Unknown process id {session_id}"),
        "unknown session error: {unknown}"
    );
}

#[tokio::test]
async fn forward_ports_reports_daemon_listen_port_conflicts_cleanly() {
    let cluster = support::spawn_cluster().await;
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();

    let error = cluster
        .broker
        .call_tool_error(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": occupied_addr.to_string(),
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;

    assert!(
        error.contains("opening tcp listener on `builder-a`")
            && error.contains(&occupied_addr.to_string()),
        "expected clean bind failure, got: {error}"
    );
}

#[tokio::test]
async fn forward_ports_relays_remote_udp_datagrams_from_two_peers_full_duplex() {
    let cluster = support::spawn_cluster().await;
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

    let open = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
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
        .send_to(b"remote-udp-a", &listen_endpoint)
        .await
        .unwrap();
    client_b
        .send_to(b"remote-udp-b", &listen_endpoint)
        .await
        .unwrap();

    let mut buf = [0u8; 64];
    let read_a = tokio::time::timeout(Duration::from_secs(2), client_a.recv_from(&mut buf))
        .await
        .expect("client a should receive udp reply")
        .unwrap()
        .0;
    assert_eq!(&buf[..read_a], b"remote-udp-a");
    let read_b = tokio::time::timeout(Duration::from_secs(2), client_b.recv_from(&mut buf))
        .await
        .expect("client b should receive udp reply")
        .unwrap()
        .0;
    assert_eq!(&buf[..read_b], b"remote-udp-b");

    let closed = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id]
            }),
        )
        .await;
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn forward_ports_reconnect_after_live_tunnel_drop_and_accept_new_tcp_connections() {
    let cluster = support::spawn_cluster().await;
    let echo_addr = support::spawn_tcp_echo().await;

    let open = cluster
        .broker
        .open_tcp_forward("builder-a", "local", "127.0.0.1:0", &echo_addr.to_string())
        .await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    support::wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "open",
        Duration::from_secs(5),
    )
    .await
    .expect("forward should stay open after reconnect");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");
}

#[tokio::test]
async fn forward_ports_reconnect_after_connect_side_tunnel_drop_and_accept_new_tcp_connections() {
    let cluster = support::spawn_cluster().await;
    let echo_addr = support::spawn_tcp_echo().await;

    let open = cluster
        .broker
        .open_tcp_forward("local", "builder-a", "127.0.0.1:0", &echo_addr.to_string())
        .await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    let mut trigger = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    trigger.write_all(b"trigger").await.unwrap();
    trigger.shutdown().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        let mut ignored = Vec::new();
        let _ = trigger.read_to_end(&mut ignored).await;
    })
    .await;

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"after").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut echoed))
        .await
        .expect("future tcp connection should succeed after connect-side reconnect")
        .unwrap();
    assert_eq!(echoed, b"after");

    tokio::time::sleep(Duration::from_millis(250)).await;

    let mut later = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    later.write_all(b"later").await.unwrap();
    later.shutdown().await.unwrap();
    let mut echoed_later = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), later.read_to_end(&mut echoed_later))
        .await
        .expect("forward should stay usable after connect-side reconnect settles")
        .unwrap();
    assert_eq!(echoed_later, b"later");

    let forward = support::wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "open",
        Duration::from_secs(5),
    )
    .await
    .expect("forward should stay open after connect-side reconnect");
    assert_eq!(forward["status"], "open");

    let closed = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id]
            }),
        )
        .await;
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn terminal_port_tunnel_corruption_releases_remote_listener_without_waiting_for_resume_timeout()
 {
    let cluster = support::spawn_cluster().await;
    let echo_addr = support::spawn_tcp_echo().await;

    let open = cluster
        .broker
        .open_tcp_forward("builder-a", "local", "127.0.0.1:0", &echo_addr.to_string())
        .await;
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.corrupt_port_tunnels().await;

    support::wait_for_daemon_listener_rebind(&listen_endpoint, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn forward_ports_reconnect_after_live_tunnel_drop_and_relays_future_udp_datagrams() {
    let cluster = support::spawn_cluster().await;
    let udp_echo = support::spawn_udp_echo().await;

    let open = cluster
        .broker
        .open_udp_forward("builder-a", "local", "127.0.0.1:0", &udp_echo.to_string())
        .await;
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender.send_to(b"after", &listen_endpoint).await.unwrap();

    let mut buf = [0u8; 16];
    let (read, _) = tokio::time::timeout(Duration::from_secs(5), sender.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..read], b"after");
}

#[tokio::test]
async fn forward_ports_release_remote_listeners_when_broker_stops() {
    let mut cluster = support::spawn_cluster().await;
    let listen_addr = support::allocate_addr();

    let open = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": listen_addr.to_string(),
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    assert_eq!(
        open.structured_content["forwards"][0]["listen_endpoint"],
        listen_addr.to_string()
    );

    cluster.broker.stop().await;

    support::wait_for_daemon_listener_rebind(&listen_addr.to_string(), Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn forward_ports_release_remote_listeners_after_broker_crash() {
    let daemon_a = support::DaemonFixture::spawn("builder-a").await;
    let daemon_b = support::DaemonFixture::spawn("builder-b").await;
    let mut broker = support::HttpBrokerFixture::spawn(&daemon_a, &daemon_b).await;
    let client = RemoteExecClient::connect(Connection::StreamableHttp {
        url: broker.url.clone(),
    })
    .await
    .unwrap();
    let listen_addr = support::allocate_addr();

    let open = client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Open {
                listen_side: "builder-a".to_string(),
                connect_side: "local".to_string(),
                forwards: vec![ForwardPortSpec {
                    listen_endpoint: listen_addr.to_string(),
                    connect_endpoint: "127.0.0.1:9".to_string(),
                    protocol: ForwardPortProtocol::Tcp,
                }],
            },
        )
        .await
        .unwrap();
    assert!(!open.is_error, "open failed: {}", open.text_output);

    tokio::time::sleep(Duration::from_millis(200)).await;
    broker.kill().await;

    support::wait_for_daemon_listener_rebind(&listen_addr.to_string(), Duration::from_secs(15))
        .await;

    let reopened_broker = support::BrokerFixture::spawn(&daemon_a, &daemon_b).await;
    let reopened = reopened_broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": listen_addr.to_string(),
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let reopened_forward_id = reopened.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        reopened.structured_content["forwards"][0]["listen_endpoint"],
        listen_addr.to_string()
    );

    let closed = reopened_broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [reopened_forward_id]
            }),
        )
        .await;
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn forward_ports_fail_cleanly_after_daemon_restart_and_can_reopen() {
    let mut cluster = support::spawn_cluster().await;
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
                        Ok(0) => return,
                        Ok(read) => read,
                        Err(_) => return,
                    };
                    if stream.write_all(&buf[..read]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    let open = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
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

    let mut before_restart = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    before_restart.write_all(b"before").await.unwrap();
    let mut echoed = [0u8; 6];
    before_restart.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"before");
    drop(before_restart);

    cluster.daemon_a.restart().await;

    let failed = support::wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "failed",
        Duration::from_secs(5),
    )
    .await;
    let failed = failed.expect("forward should fail after daemon restart");
    assert_eq!(failed["status"], "failed");
    let last_error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        last_error.contains("daemon transport error")
            || last_error.contains("port tunnel closed")
            || last_error.contains("reading tcp listen tunnel")
            || last_error.contains("port_bind_closed")
            || last_error.contains("port_accept_failed")
            || last_error.contains("unknown_port_tunnel_session"),
        "unexpected restart failure error: {last_error}"
    );

    wait_for_stale_forward_to_stop_accepting(&listen_endpoint, Duration::from_secs(12)).await;

    let reopened = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let reopened_id = reopened.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let reopened_endpoint = reopened.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    let mut after_reopen = tokio::net::TcpStream::connect(&reopened_endpoint)
        .await
        .unwrap();
    after_reopen.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    after_reopen.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");

    let closed = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [reopened_id]
            }),
        )
        .await;
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_command_preserves_output_after_external_pipeline_steps() {
    let cluster = support::spawn_cluster().await;

    let result = cluster
        .broker
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf 'marker\\n'; printf 'external\\n' | cat; printf 'done\\n'",
                "shell": "/bin/sh",
                "tty": false,
                "yield_time_ms": 10_000
            }),
        )
        .await;

    assert_eq!(result.structured_content["target"], "builder-a");
    assert_eq!(result.structured_content["exit_code"], 0);
    assert_eq!(
        result.structured_content["output"],
        serde_json::Value::String("marker\nexternal\ndone\n".to_string())
    );
}

#[tokio::test]
async fn transfer_files_copies_local_file_to_remote_exact_destination_path() {
    let cluster = support::spawn_cluster().await;
    let local_dir = tempfile::tempdir().unwrap();
    let source = local_dir.path().join("artifact.txt");
    std::fs::write(&source, "artifact\n").unwrap();
    let destination = cluster.daemon_a.workdir.join("releases/current.txt");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "builder-a",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "artifact\n");
    assert_eq!(
        result.structured_content["destination"]["target"],
        "builder-a"
    );
    assert!(
        !cluster
            .daemon_a
            .workdir
            .join("releases/artifact.txt")
            .exists()
    );
}

#[tokio::test]
async fn transfer_files_copies_remote_file_back_to_local() {
    let cluster = support::spawn_cluster().await;
    let source = cluster.daemon_a.workdir.join("build.log");
    std::fs::write(&source, "done\n").unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    let destination = local_dir.path().join("logs/build.log");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "done\n");
    assert_eq!(result.structured_content["source"]["target"], "builder-a");
}

#[tokio::test]
async fn transfer_files_moves_remote_directory_between_targets_without_basename_inference() {
    let cluster = support::spawn_cluster().await;
    let source_root = cluster.daemon_a.workdir.join("dist");
    std::fs::create_dir_all(source_root.join("empty")).unwrap();
    std::fs::create_dir_all(source_root.join("bin")).unwrap();
    std::fs::write(source_root.join("bin/tool.sh"), "#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();
    }
    let destination = cluster.daemon_b.workdir.join("release");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source_root.display().to_string()
                },
                "destination": {
                    "target": "builder-b",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(destination.join("empty").is_dir());
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(destination.join("bin/tool.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    assert!(!destination.join("dist").exists());
    assert_eq!(result.structured_content["source_type"], "directory");
}

#[tokio::test]
async fn transfer_files_bundles_multiple_local_sources_with_zstd_for_remote_destination() {
    let cluster = support::spawn_cluster().await;
    let local_dir = tempfile::tempdir().unwrap();
    let file_source = local_dir.path().join("alpha.txt");
    let directory_source = local_dir.path().join("tree");
    std::fs::write(&file_source, "alpha\n").unwrap();
    std::fs::create_dir_all(&directory_source).unwrap();
    std::fs::write(directory_source.join("nested.txt"), "nested\n").unwrap();
    let destination = cluster.daemon_a.workdir.join("bundle");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "sources": [
                    {
                        "target": "local",
                        "path": file_source.display().to_string()
                    },
                    {
                        "target": "local",
                        "path": directory_source.display().to_string()
                    }
                ],
                "destination": {
                    "target": "builder-a",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(
        std::fs::read_to_string(destination.join("alpha.txt")).unwrap(),
        "alpha\n"
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("tree/nested.txt")).unwrap(),
        "nested\n"
    );
    assert_eq!(result.structured_content["source_type"], "multiple");
    assert_eq!(
        result.structured_content["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

async fn wait_for_stale_forward_to_stop_accepting(endpoint: &str, timeout: Duration) {
    let started = std::time::Instant::now();
    loop {
        let result = tokio::time::timeout(
            Duration::from_millis(250),
            tokio::net::TcpStream::connect(endpoint),
        )
        .await;
        if result.is_err() || result.unwrap().is_err() {
            return;
        }
        if started.elapsed() >= timeout {
            panic!("stale forwarded listener at {endpoint} kept accepting after daemon restart");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
