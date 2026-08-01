#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use remote_exec_proto::public::{ExecCommandInput, WriteStdinInput};
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortsInput};
use support::cpp_daemon::{
    CppDaemonBrokerFixture, CrashableCppDaemonBrokerFixture, wait_for_forward_ready,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn broker_forwards_ports_through_real_cpp_daemon_and_handles_port_conflicts() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };
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

    let open = fixture.open_tcp_forward(&echo_addr.to_string()).await;
    assert!(!open.is_error, "open failed: {}", open.text_output);
    let opened = &open.structured_content["forwards"][0];
    assert_eq!(opened["phase"], "ready");
    assert_eq!(opened["listen_state"]["generation"], 1);
    assert_eq!(opened["connect_state"]["generation"], 1);
    let opened_forward_id = opened["forward_id"].as_str().unwrap().to_string();
    let opened_listen_endpoint = opened["listen_endpoint"].as_str().unwrap().to_string();

    let mut stream = tokio::net::TcpStream::connect(&opened_listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"cpp-forward").await.unwrap();
    let mut echoed = [0u8; 11];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"cpp-forward");

    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();
    let occupied_open = fixture
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Open {
                listen_side: "builder-cpp".to_string(),
                connect_side: "local".to_string(),
                forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                    listen_endpoint: occupied_addr.to_string(),
                    connect_endpoint: echo_addr.to_string(),
                    protocol: ForwardPortProtocol::Tcp,
                }],
            },
        )
        .await
        .unwrap();
    assert!(occupied_open.is_error, "expected occupied port failure");
    assert!(
        occupied_open
            .text_output
            .contains("opening tcp listener on `builder-cpp`")
            && occupied_open
                .text_output
                .contains(&occupied_addr.to_string()),
        "unexpected occupied port error: {}",
        occupied_open.text_output
    );

    let close = fixture.close_forward(opened_forward_id).await;
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn list_targets_reports_version_checked_port_forward_support_for_real_cpp_daemon() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };
    let result = fixture
        .client
        .call_tool("list_targets", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        !result.is_error,
        "list_targets failed: {}",
        result.text_output
    );
    assert_eq!(result.structured_content["targets"][0]["healthy"], true);
    assert_eq!(
        result.structured_content["targets"][0]["daemon_info"]["supports_port_forward"],
        true
    );
    assert!(
        result.structured_content["targets"][0]["daemon_info"]
            .get("port_forward_protocol_version")
            .is_none()
    );
}

#[tokio::test]
async fn broker_prunes_cpp_exec_sessions_when_daemon_limit_is_reached() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn_with_daemon_config(
        "max_open_sessions = 2\n\
yield_time_exec_command_default_ms = 1\n\
yield_time_exec_command_max_ms = 1000\n\
yield_time_exec_command_min_ms = 1\n\
yield_time_write_stdin_poll_default_ms = 1\n\
yield_time_write_stdin_poll_max_ms = 1000\n\
yield_time_write_stdin_poll_min_ms = 1\n\
yield_time_write_stdin_input_default_ms = 1\n\
yield_time_write_stdin_input_max_ms = 1000\n\
yield_time_write_stdin_input_min_ms = 1\n",
    )
    .await
    else {
        return;
    };

    let first = fixture
        .client
        .call_tool(
            "exec_command",
            &exec_request(&session_limit_command("first")),
        )
        .await
        .unwrap();
    assert!(!first.is_error, "first exec failed: {}", first.text_output);
    let first_session_id = first.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let second = fixture
        .client
        .call_tool(
            "exec_command",
            &exec_request(&session_limit_command("second")),
        )
        .await
        .unwrap();
    assert!(
        !second.is_error,
        "second exec failed: {}",
        second.text_output
    );
    let second_session_id = second.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let third = fixture
        .client
        .call_tool(
            "exec_command",
            &exec_request(&session_limit_command("third")),
        )
        .await
        .unwrap();
    assert!(!third.is_error, "third exec failed: {}", third.text_output);
    let third_session_id = third.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let first_poll = fixture
        .client
        .call_tool("write_stdin", &poll_request(&first_session_id))
        .await
        .unwrap();
    assert!(first_poll.is_error, "expected pruned session failure");
    support::assert_correlated_direct_tool_error(
        &first_poll.text_output,
        "write_stdin",
        Some("builder-cpp"),
        &format!("write_stdin failed: Unknown process id {first_session_id}"),
    );

    let second_poll = fixture
        .client
        .call_tool("write_stdin", &poll_request(&second_session_id))
        .await
        .unwrap();
    assert!(
        !second_poll.is_error,
        "second poll failed: {}",
        second_poll.text_output
    );
    assert_eq!(second_poll.structured_content["target"], "builder-cpp");

    let third_poll = fixture
        .client
        .call_tool("write_stdin", &poll_request(&third_session_id))
        .await
        .unwrap();
    assert!(
        !third_poll.is_error,
        "third poll failed: {}",
        third_poll.text_output
    );
    assert_eq!(third_poll.structured_content["target"], "builder-cpp");
}

#[tokio::test]
async fn broker_forwards_udp_datagrams_through_real_cpp_daemon_full_duplex() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };
    let echo_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        for _ in 0..2 {
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
        .open_forward(
            "builder-cpp",
            "local",
            "127.0.0.1:0".to_string(),
            echo_addr.to_string(),
            ForwardPortProtocol::Udp,
        )
        .await;
    assert!(!open.is_error, "open failed: {}", open.text_output);
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
        .send_to(b"cpp-udp-a", &listen_endpoint)
        .await
        .unwrap();
    client_b
        .send_to(b"cpp-udp-b", &listen_endpoint)
        .await
        .unwrap();

    let mut buf = [0u8; 1024];
    let read_a = tokio::time::timeout(Duration::from_secs(5), client_a.recv(&mut buf))
        .await
        .expect("client a should receive udp reply")
        .unwrap();
    assert_eq!(&buf[..read_a], b"cpp-udp-a");
    let read_b = tokio::time::timeout(Duration::from_secs(5), client_b.recv(&mut buf))
        .await
        .expect("client b should receive udp reply")
        .unwrap();
    assert_eq!(&buf[..read_b], b"cpp-udp-b");

    let close = fixture.close_forward(forward_id).await;
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn cpp_forward_ports_reconnect_after_tunnel_drop() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };
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

    let open = fixture.open_tcp_forward(&echo_addr.to_string()).await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    fixture.drop_port_tunnels().await;

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");
    let forward =
        wait_for_forward_ready(&fixture.client, &forward_id, Duration::from_secs(5)).await;
    assert_eq!(forward["status"], "open");
    assert_eq!(forward["phase"], "ready");

    let close = fixture.close_forward(forward_id).await;
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn cpp_forward_ports_reconnect_after_connect_tunnel_drop() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match echo_listener.accept().await {
                Ok(value) => value,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
            });
        }
    });

    let open = fixture
        .open_tcp_forward_local_to_cpp(&echo_addr.to_string())
        .await;
    assert!(!open.is_error, "open failed: {}", open.text_output);
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    fixture.drop_port_tunnels().await;

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

    let forward =
        wait_for_forward_ready(&fixture.client, &forward_id, Duration::from_secs(5)).await;
    assert_eq!(forward["status"], "open");
    assert_eq!(forward["phase"], "ready");

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

    let close = fixture.close_forward(forward_id).await;
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn real_cpp_daemon_releases_listener_after_broker_crash() {
    let Some(mut fixture) = CrashableCppDaemonBrokerFixture::spawn().await else {
        return;
    };

    let open = fixture
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Open {
                listen_side: "builder-cpp".to_string(),
                connect_side: "local".to_string(),
                forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                    listen_endpoint: "127.0.0.1:0".to_string(),
                    connect_endpoint: "127.0.0.1:9".to_string(),
                    protocol: ForwardPortProtocol::Tcp,
                }],
            },
        )
        .await
        .unwrap();
    assert!(!open.is_error, "open failed: {}", open.text_output);
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .expect("listen endpoint")
        .to_string();
    assert_ne!(listen_endpoint, "127.0.0.1:0");

    fixture.kill_broker().await;

    let (reopened_client, reopened_forward_id) = fixture
        .wait_for_public_forward_reopen(&listen_endpoint, Duration::from_secs(10))
        .await;

    let closed = reopened_client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Close {
                forward_ids: vec![reopened_forward_id.into()],
            },
        )
        .await
        .unwrap();
    assert!(!closed.is_error, "close failed: {}", closed.text_output);
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
#[cfg(not(windows))]
async fn windows_cpp_daemon_smoke() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };

    let target_info = fixture
        .client
        .call_tool("list_targets", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        !target_info.is_error,
        "list_targets failed: {}",
        target_info.text_output
    );
    assert_eq!(
        target_info.structured_content["targets"][0]["healthy"],
        true
    );
    assert_eq!(
        target_info.structured_content["targets"][0]["daemon_info"]["supports_port_forward"],
        true
    );
    assert!(
        target_info.structured_content["targets"][0]["daemon_info"]
            .get("port_forward_protocol_version")
            .is_none()
    );

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

    let open = fixture.open_tcp_forward(&echo_addr.to_string()).await;
    assert!(!open.is_error, "open failed: {}", open.text_output);
    let opened = &open.structured_content["forwards"][0];
    let forward_id = opened["forward_id"].as_str().unwrap().to_string();
    let listen_endpoint = opened["listen_endpoint"].as_str().unwrap().to_string();

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"windows-cpp-forward").await.unwrap();
    let mut echoed = [0u8; 19];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"windows-cpp-forward");

    let close = fixture.close_forward(forward_id).await;
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

fn exec_request(cmd: &str) -> ExecCommandInput {
    ExecCommandInput {
        target: "builder-cpp".to_string(),
        cmd: cmd.to_string(),
        workdir: None,
        shell: Some(test_shell().to_string()),
        tty: false,
        yield_time_ms: Some(1),
        max_output_tokens: None,
        login: Some(false),
    }
}

fn poll_request(session_id: &str) -> WriteStdinInput {
    WriteStdinInput {
        session_id: session_id.to_string(),
        chars: Some(String::new()),
        yield_time_ms: Some(1),
        max_output_tokens: None,
        pty_size: None,
        target: None,
    }
}

#[cfg(windows)]
fn session_limit_command(label: &str) -> String {
    format!("echo {label} & ping -n 30 127.0.0.1 >nul")
}

#[cfg(not(windows))]
fn session_limit_command(label: &str) -> String {
    format!("printf {label}; sleep 30")
}

#[cfg(windows)]
fn test_shell() -> &'static str {
    "cmd.exe"
}

#[cfg(not(windows))]
fn test_shell() -> &'static str {
    "/bin/sh"
}
