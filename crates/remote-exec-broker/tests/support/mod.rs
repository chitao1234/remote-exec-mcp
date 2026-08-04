#![allow(dead_code)]

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod blocking_child_process;
pub mod certs;
pub mod cpp_daemon;
pub mod fixture;
pub mod spawners;
pub mod streamable_http_child;
pub mod stub_daemon;
pub mod tunnel_drop_proxy;

#[allow(
    unused_imports,
    reason = "Different broker integration tests use different shared helper subsets"
)]
pub mod test_helpers {
    pub use remote_exec_test_support::test_helpers::*;
}

#[allow(
    unused_imports,
    reason = "Different broker integration tests use different shared helper subsets"
)]
pub mod transfer_archive {
    pub use remote_exec_test_support::transfer_archive::*;
}

#[allow(
    unused_imports,
    reason = "Some broker integration test crates use this root re-export"
)]
pub use spawners::spawn_broker_with_plain_http_stub_daemon;

pub fn assert_correlated_tool_error(
    error: &str,
    tool: &str,
    target: Option<&str>,
    expected_suffix: &str,
) {
    let tool = if tool.starts_with("remote_") {
        tool.to_string()
    } else {
        format!("remote_{tool}")
    };
    assert_correlated_tool_error_named(error, &tool, target, expected_suffix);
}

pub fn assert_correlated_direct_tool_error(
    error: &str,
    tool: &str,
    target: Option<&str>,
    expected_suffix: &str,
) {
    assert_correlated_tool_error_named(error, tool, target, expected_suffix);
}

fn assert_correlated_tool_error_named(
    error: &str,
    tool: &str,
    target: Option<&str>,
    expected_suffix: &str,
) {
    assert!(
        error.starts_with("request_id=req_"),
        "missing request_id prefix in error: {error}"
    );
    assert!(
        error.contains(&format!(" tool={tool}")),
        "missing tool={tool} in error: {error}"
    );
    match target {
        Some(target) => assert!(
            error.contains(&format!(" target={target}: ")),
            "missing target={target} in error: {error}"
        ),
        None => assert!(
            !error.contains(" target="),
            "unexpected target context in error: {error}"
        ),
    }
    assert!(
        error.ends_with(expected_suffix),
        "error did not preserve expected suffix `{expected_suffix}`: {error}"
    );
}

/// Asserts that a TCP stream read result represents a closed stream: EOF or one
/// of the connection-teardown error kinds daemons and brokers surface on close.
pub fn assert_stream_closed(result: std::io::Result<usize>, what: &str) {
    match result {
        Ok(0) => {}
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Ok(read) => panic!("expected {what} to close, read {read} byte(s) instead"),
        Err(err) => panic!("unexpected {what} read error: {err}"),
    }
}

/// Spawns a TCP echo server on an ephemeral port, echoing every received byte
/// back to its client, until the listener is dropped.
pub async fn spawn_tcp_echo() -> std::net::SocketAddr {
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
    echo_addr
}

/// Spawns a UDP echo server on an ephemeral port, echoing every received
/// datagram back to its peer, until the socket is dropped.
pub async fn spawn_udp_echo() -> std::net::SocketAddr {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        loop {
            let (read, peer) = match socket.recv_from(&mut buf).await {
                Ok(value) => value,
                Err(_) => return,
            };
            if socket.send_to(&buf[..read], peer).await.is_err() {
                return;
            }
        }
    });
    addr
}

/// Keeps daemon/broker child logging quiet unless the test explicitly opts in.
pub fn apply_quiet_test_logging(command: &mut tokio::process::Command) {
    if std::env::var_os("REMOTE_EXEC_LOG").is_some() || std::env::var_os("RUST_LOG").is_some() {
        return;
    }

    let filter = std::env::var("REMOTE_EXEC_TEST_LOG").unwrap_or_else(|_| "error".to_string());
    command.env("REMOTE_EXEC_LOG", filter);
}

/// Polls a plain HTTP daemon's `/v1/health` until it responds or `timeout` elapses.
pub async fn wait_until_ready_http(
    addr: std::net::SocketAddr,
    timeout: Duration,
    poll: Duration,
    description: &str,
) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(timeout, async {
        loop {
            if client
                .post(format!("http://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(poll).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{description} at http://{addr} did not become ready within {timeout:?}")
    });
}

/// Polls a streamable HTTP MCP broker's initialize endpoint until it responds
/// successfully or `timeout` elapses.
pub async fn wait_until_ready_mcp_http(
    url: &str,
    timeout: Duration,
    poll: Duration,
    description: &str,
) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(timeout, async {
        loop {
            let response = client
                .post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
                .send()
                .await;
            if response
                .as_ref()
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            tokio::time::sleep(poll).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{description} at {url} did not become ready within {timeout:?}")
    });
}
