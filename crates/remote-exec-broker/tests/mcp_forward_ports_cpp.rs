#[path = "support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::Duration;

use remote_exec_broker::client::{Connection, RemoteExecClient};
#[cfg(unix)]
use remote_exec_proto::public::{ExecCommandInput, WriteStdinInput};
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortsInput};
use std::io::Write;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};

const CPP_READY_TIMEOUT: Duration = Duration::from_secs(20);
const CPP_READY_POLL: Duration = Duration::from_millis(50);
static MISSING_CPP_DAEMON_WARNING: Once = Once::new();

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn cpp_config_path(path: &Path) -> String {
    path.display().to_string()
}

fn apply_quiet_test_logging(command: &mut tokio::process::Command) {
    if std::env::var_os("REMOTE_EXEC_LOG").is_some() || std::env::var_os("RUST_LOG").is_some() {
        return;
    }

    let filter = std::env::var("REMOTE_EXEC_TEST_LOG").unwrap_or_else(|_| "error".to_string());
    command.env("REMOTE_EXEC_LOG", filter);
}

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
async fn list_targets_reports_port_forward_protocol_version_for_real_cpp_daemon() {
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
    assert_eq!(
        result.structured_content["targets"][0]["daemon_info"]["port_forward_protocol_version"],
        4
    );
}

#[cfg(unix)]
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
        .call_tool("exec_command", &exec_request("printf first; sleep 30"))
        .await
        .unwrap();
    assert!(!first.is_error, "first exec failed: {}", first.text_output);
    let first_session_id = first.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let second = fixture
        .client
        .call_tool("exec_command", &exec_request("printf second; sleep 30"))
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
        .call_tool("exec_command", &exec_request("printf third; sleep 30"))
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
    support::assert_correlated_tool_error(
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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
                forward_ids: vec![reopened_forward_id],
            },
        )
        .await
        .unwrap();
    assert!(!closed.is_error, "close failed: {}", closed.text_output);
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[cfg(windows)]
#[tokio::test]
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
        target_info.structured_content["targets"][0]["daemon_info"]["port_forward_protocol_version"],
        4
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

struct CppDaemonBrokerFixture {
    _tempdir: TempDir,
    client: RemoteExecClient,
    proxy: TunnelDropProxy,
    daemon: tokio::process::Child,
}

impl CppDaemonBrokerFixture {
    async fn spawn() -> Option<Self> {
        Self::spawn_with_daemon_config("").await
    }

    async fn spawn_with_daemon_config(extra_daemon_config: &str) -> Option<Self> {
        let daemon_binary = match cpp_daemon_binary() {
            Some(path) => path,
            None => {
                warn_missing_cpp_daemon_executable();
                return None;
            }
        };
        remote_exec_broker::install_crypto_provider().unwrap();

        let tempdir = tempfile::tempdir().unwrap();
        let daemon_binary = stage_cpp_daemon_binary(&daemon_binary, tempdir.path());
        let broker_config = tempdir.path().join("broker.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        std::fs::create_dir_all(&daemon_workdir).unwrap();

        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        let daemon_config_body = format!(
            "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n{}",
            cpp_config_path(&daemon_workdir),
            cpp_config_path(&daemon_bound_addr_file),
            extra_daemon_config
        );
        let (daemon, backend_addr) = spawn_cpp_daemon_with_bound_addr(
            &daemon_binary,
            &daemon_config,
            &daemon_bound_addr_file,
            daemon_config_body,
        )
        .await;
        let proxy = TunnelDropProxy::spawn(backend_addr).await;
        let daemon_addr = proxy.listen_addr;

        std::fs::write(
            &broker_config,
            format!(
                r#"[targets.builder-cpp]
base_url = "http://{}"
allow_insecure_http = true
expected_daemon_name = "builder-cpp"

[local]
default_workdir = {}
pty = "none"
"#,
                daemon_addr,
                toml_string(&tempdir.path().join("local-work").display().to_string())
            ),
        )
        .unwrap();
        std::fs::create_dir_all(tempdir.path().join("local-work")).unwrap();

        let client = RemoteExecClient::connect(Connection::Config {
            config_path: broker_config,
        })
        .await
        .unwrap();

        Some(Self {
            _tempdir: tempdir,
            client,
            proxy,
            daemon,
        })
    }

    async fn open_forward(
        &self,
        listen_side: &str,
        connect_side: &str,
        listen_endpoint: String,
        connect_endpoint: String,
        protocol: ForwardPortProtocol,
    ) -> remote_exec_broker::client::ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Open {
                    listen_side: listen_side.to_string(),
                    connect_side: connect_side.to_string(),
                    forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                        listen_endpoint,
                        connect_endpoint,
                        protocol,
                    }],
                },
            )
            .await
            .unwrap()
    }

    async fn open_tcp_forward(
        &self,
        connect_endpoint: &str,
    ) -> remote_exec_broker::client::ToolResponse {
        self.open_forward(
            "builder-cpp",
            "local",
            "127.0.0.1:0".to_string(),
            connect_endpoint.to_string(),
            ForwardPortProtocol::Tcp,
        )
        .await
    }

    #[cfg(unix)]
    async fn open_tcp_forward_local_to_cpp(
        &self,
        connect_endpoint: &str,
    ) -> remote_exec_broker::client::ToolResponse {
        self.open_forward(
            "local",
            "builder-cpp",
            "127.0.0.1:0".to_string(),
            connect_endpoint.to_string(),
            ForwardPortProtocol::Tcp,
        )
        .await
    }

    async fn close_forward(&self, forward_id: String) -> remote_exec_broker::client::ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Close {
                    forward_ids: vec![forward_id],
                },
            )
            .await
            .unwrap()
    }

    #[cfg(unix)]
    async fn drop_port_tunnels(&self) {
        self.proxy.drop_port_tunnels().await;
    }
}

fn cpp_daemon_skip_message() -> String {
    let default = cpp_daemon_default_binary();
    format!(
        "set REMOTE_EXEC_CPP_DAEMON or build {} before running this test",
        default.display()
    )
}

fn warn_missing_cpp_daemon_executable() {
    MISSING_CPP_DAEMON_WARNING.call_once(|| {
        let env_path = std::env::var_os("REMOTE_EXEC_CPP_DAEMON")
            .map(PathBuf::from)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<not set>".to_string());
        let default = cpp_daemon_default_binary();
        let message = format!(
            "\n\
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\
!!! WARNING: SKIPPING REAL C++ DAEMON INTEGRATION TESTS                 !!!\n\
!!!                                                                        !!!\n\
!!! The remote-exec-daemon-cpp executable is not available. These Rust     !!!\n\
!!! integration tests are being skipped instead of exercising the real     !!!\n\
!!! C++ daemon.                                                           !!!\n\
!!!                                                                        !!!\n\
!!! REMOTE_EXEC_CPP_DAEMON = {env_path}\n\
!!! default checked path     = {default_path}\n\
!!!                                                                        !!!\n\
!!! {skip_message}\n\
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n",
            default_path = default.display(),
            skip_message = cpp_daemon_skip_message()
        );
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(message.as_bytes());
        let _ = stderr.flush();
    });
}

impl Drop for CppDaemonBrokerFixture {
    fn drop(&mut self) {
        self.proxy.stop();
        let _ = self.daemon.start_kill();
    }
}

#[cfg(unix)]
async fn wait_for_forward_ready(
    client: &RemoteExecClient,
    forward_id: &str,
    timeout: Duration,
) -> serde_json::Value {
    let started = std::time::Instant::now();
    loop {
        let response = client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::List {
                    forward_ids: vec![forward_id.to_string()],
                    listen_side: None,
                    connect_side: None,
                },
            )
            .await
            .unwrap();
        let entry = response.structured_content["forwards"][0].clone();
        if entry["status"] == "open" && entry["phase"] == "ready" {
            return entry;
        }
        if started.elapsed() >= timeout {
            panic!("forward `{forward_id}` did not become ready within {timeout:?}; last={entry}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

struct TunnelDropProxy {
    listen_addr: std::net::SocketAddr,
    #[cfg_attr(windows, allow(dead_code))]
    active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
    background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(unix)]
struct CrashableCppDaemonBrokerFixture {
    _tempdir: TempDir,
    broker_config: PathBuf,
    client: RemoteExecClient,
    broker: tokio::process::Child,
    daemon: tokio::process::Child,
}

#[cfg(unix)]
impl CrashableCppDaemonBrokerFixture {
    async fn spawn() -> Option<Self> {
        let daemon_binary = match cpp_daemon_binary() {
            Some(path) => path,
            None => {
                warn_missing_cpp_daemon_executable();
                return None;
            }
        };
        remote_exec_broker::install_crypto_provider().unwrap();

        let tempdir = tempfile::tempdir().unwrap();
        let daemon_binary = stage_cpp_daemon_binary(&daemon_binary, tempdir.path());
        let broker_config = tempdir.path().join("broker-http.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        let broker_bound_addr_file = tempdir.path().join("broker-bound-addr.txt");
        let daemon_config_body = format!(
            "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n",
            cpp_config_path(&daemon_workdir),
            cpp_config_path(&daemon_bound_addr_file)
        );
        let (daemon, daemon_addr) = spawn_cpp_daemon_with_bound_addr(
            &daemon_binary,
            &daemon_config,
            &daemon_bound_addr_file,
            daemon_config_body,
        )
        .await;

        std::fs::write(
            &broker_config,
            format!(
                r#"[targets.builder-cpp]
base_url = "http://{}"
allow_insecure_http = true
expected_daemon_name = "builder-cpp"

[local]
default_workdir = {}
pty = "none"

[mcp]
transport = "streamable_http"
listen = "127.0.0.1:0"
path = "/mcp"
"#,
                daemon_addr,
                toml_string(&tempdir.path().join("local-work").display().to_string())
            ),
        )
        .unwrap();
        std::fs::create_dir_all(tempdir.path().join("local-work")).unwrap();

        let mut broker = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        broker.arg(&broker_config);
        broker.env(
            "REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE",
            &broker_bound_addr_file,
        );
        apply_quiet_test_logging(&mut broker);
        broker.kill_on_drop(true);
        let broker = broker.spawn().unwrap();
        let broker_addr = wait_for_bound_addr_file(&broker_bound_addr_file, "C++ broker").await;
        let broker_url = format!("http://{broker_addr}/mcp");
        wait_until_ready_mcp_http(&broker_url).await;
        let client = RemoteExecClient::connect(Connection::StreamableHttp { url: broker_url })
            .await
            .unwrap();

        Some(Self {
            _tempdir: tempdir,
            broker_config,
            client,
            broker,
            daemon,
        })
    }

    async fn kill_broker(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.broker.wait().await;
    }

    async fn wait_for_public_forward_reopen(
        &self,
        endpoint: &str,
        timeout: Duration,
    ) -> (RemoteExecClient, String) {
        const CPP_PUBLIC_REOPEN_POLL: Duration = Duration::from_millis(200);

        let started = std::time::Instant::now();
        loop {
            let client = RemoteExecClient::connect(Connection::Config {
                config_path: self.broker_config.clone(),
            })
            .await
            .unwrap();
            let response = client
                .call_tool(
                    "forward_ports",
                    &ForwardPortsInput::Open {
                        listen_side: "builder-cpp".to_string(),
                        connect_side: "local".to_string(),
                        forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                            listen_endpoint: endpoint.to_string(),
                            connect_endpoint: "127.0.0.1:9".to_string(),
                            protocol: ForwardPortProtocol::Tcp,
                        }],
                    },
                )
                .await
                .unwrap();
            if !response.is_error {
                assert_eq!(
                    response.structured_content["forwards"][0]["listen_endpoint"],
                    endpoint
                );
                let forward_id = response.structured_content["forwards"][0]["forward_id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                return (client, forward_id);
            }
            if !response
                .text_output
                .contains("opening tcp listener on `builder-cpp`")
            {
                panic!(
                    "unexpected public reopen failure while waiting for {endpoint}: {}",
                    response.text_output
                );
            }
            if started.elapsed() >= timeout {
                panic!(
                    "C++ daemon listener on {endpoint} was not released within {timeout:?}; last error={}",
                    response.text_output
                );
            }
            tokio::time::sleep(CPP_PUBLIC_REOPEN_POLL).await;
        }
    }
}

#[cfg(unix)]
impl Drop for CrashableCppDaemonBrokerFixture {
    fn drop(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.daemon.start_kill();
    }
}

impl TunnelDropProxy {
    async fn spawn(daemon_addr: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let active_port_tunnels = Arc::new(Mutex::new(Vec::new()));
        let background_tasks = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let active_port_tunnels_task = active_port_tunnels.clone();
        let background_tasks_accept = background_tasks.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let (stream, _) = match accepted {
                            Ok(value) => value,
                            Err(_) => break,
                        };
                        let active_port_tunnels = active_port_tunnels_task.clone();
                        let connection_handle = tokio::spawn(async move {
                            if let Err(err) = proxy_connection(stream, daemon_addr, active_port_tunnels).await {
                                if is_expected_proxy_teardown_error(&err) {
                                    return;
                                }
                                panic!("C++ tunnel-drop proxy connection failed: {err}");
                            }
                        });
                        background_tasks_accept.lock().await.push(connection_handle);
                    }
                }
            }
        });

        Self {
            listen_addr,
            active_port_tunnels,
            background_tasks,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    #[cfg(unix)]
    async fn drop_port_tunnels(&self) {
        let mut active = self.active_port_tunnels.lock().await;
        for shutdown in active.drain(..) {
            let _ = shutdown.send(());
        }
        drop(active);
        self.assert_no_task_panics().await;
    }

    #[cfg(unix)]
    async fn assert_no_task_panics(&self) {
        let finished = {
            let mut tasks = self.background_tasks.lock().await;
            let mut finished = Vec::new();
            let mut pending = Vec::with_capacity(tasks.len());
            for handle in tasks.drain(..) {
                if handle.is_finished() {
                    finished.push(handle);
                } else {
                    pending.push(handle);
                }
            }
            *tasks = pending;
            finished
        };

        for handle in finished {
            handle.await.expect("C++ tunnel-drop proxy task panicked");
        }
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        if let Ok(mut tasks) = self.background_tasks.try_lock() {
            for handle in tasks.drain(..) {
                handle.abort();
            }
        }
    }
}

async fn proxy_connection(
    mut client_stream: tokio::net::TcpStream,
    daemon_addr: std::net::SocketAddr,
    active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
) -> std::io::Result<()> {
    let mut backend_stream = tokio::net::TcpStream::connect(daemon_addr).await?;
    let mut request = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let read = client_stream.read(&mut byte).await?;
        if read == 0 {
            return Ok(());
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let request_text = String::from_utf8_lossy(&request);
    let is_port_tunnel = is_port_tunnel_upgrade_request(&request_text);

    backend_stream.write_all(&request).await?;

    if is_port_tunnel {
        let (drop_tx, drop_rx) = oneshot::channel();
        active_port_tunnels.lock().await.push(drop_tx);
        proxy_port_tunnel_streams(client_stream, backend_stream, drop_rx).await
    } else {
        proxy_plain_streams(client_stream, backend_stream).await
    }
}

fn is_port_tunnel_upgrade_request(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    let first_line = lower.lines().next().unwrap_or_default();
    first_line.starts_with("post /v1/port/tunnel ")
        && lower.contains("\r\nconnection: upgrade\r\n")
        && lower.contains("\r\nupgrade: remote-exec-port-tunnel\r\n")
}

async fn proxy_plain_streams(
    mut client_stream: tokio::net::TcpStream,
    mut backend_stream: tokio::net::TcpStream,
) -> std::io::Result<()> {
    let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream).await?;
    Ok(())
}

async fn proxy_port_tunnel_streams(
    mut client_stream: tokio::net::TcpStream,
    mut backend_stream: tokio::net::TcpStream,
    mut drop_rx: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream) => {
            let _ = result?;
        }
        _ = &mut drop_rx => {
            let _ = client_stream.shutdown().await;
            let _ = backend_stream.shutdown().await;
        }
    }

    Ok(())
}

fn is_expected_proxy_teardown_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    ) || matches!(err.raw_os_error(), Some(10053 | 10054 | 10061))
}

fn cpp_daemon_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("REMOTE_EXEC_CPP_DAEMON").map(PathBuf::from) {
        return path.exists().then_some(path);
    }

    let path = cpp_daemon_default_binary();
    path.exists().then_some(path)
}

fn cpp_daemon_default_binary() -> PathBuf {
    cpp_daemon_dir().join(if cfg!(windows) {
        "build/remote-exec-daemon-cpp.exe"
    } else {
        "build/remote-exec-daemon-cpp"
    })
}

fn cpp_daemon_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../remote-exec-daemon-cpp")
}

fn stage_cpp_daemon_binary(source: &Path, tempdir: &Path) -> PathBuf {
    let staged_name = if cfg!(windows) {
        "remote-exec-daemon-cpp.exe"
    } else {
        "remote-exec-daemon-cpp"
    };
    let staged = tempdir.join(staged_name);
    std::fs::copy(source, &staged).unwrap();
    staged
}

#[cfg(unix)]
fn exec_request(cmd: &str) -> ExecCommandInput {
    ExecCommandInput {
        target: "builder-cpp".to_string(),
        cmd: cmd.to_string(),
        workdir: None,
        shell: None,
        tty: false,
        yield_time_ms: Some(1),
        max_output_tokens: None,
        login: None,
    }
}

#[cfg(unix)]
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

async fn spawn_cpp_daemon_process(command: &mut tokio::process::Command) -> tokio::process::Child {
    const ETXTBSY: i32 = 26;
    for attempt in 0..5 {
        match command.spawn() {
            Ok(child) => return child,
            Err(error) if error.raw_os_error() == Some(ETXTBSY) && attempt + 1 < 5 => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("failed to spawn staged C++ daemon: {error}"),
        }
    }
    unreachable!("spawn retry loop returns or panics");
}

async fn wait_for_bound_addr_file(path: &Path, resource: &str) -> std::net::SocketAddr {
    let started = std::time::Instant::now();
    loop {
        let last = match tokio::fs::read_to_string(path).await {
            Ok(value) => match value.trim().parse() {
                Ok(addr) => return addr,
                Err(err) => format!("invalid address `{}`: {err}", value.trim()),
            },
            Err(err) => err.to_string(),
        };
        if started.elapsed() >= Duration::from_secs(5) {
            panic!(
                "{resource} did not write bound address file {}; last={last}",
                path.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn spawn_cpp_daemon_with_bound_addr(
    daemon_binary: &Path,
    daemon_config: &Path,
    bound_addr_file: &Path,
    config_body: String,
) -> (tokio::process::Child, std::net::SocketAddr) {
    std::fs::write(daemon_config, config_body).unwrap();
    let mut daemon = tokio::process::Command::new(daemon_binary);
    daemon.arg(daemon_config);
    apply_quiet_test_logging(&mut daemon);
    let child = spawn_cpp_daemon_process(&mut daemon).await;
    let daemon_addr = wait_for_bound_addr_file(bound_addr_file, "C++ daemon").await;
    wait_until_ready_http(daemon_addr).await;
    (child, daemon_addr)
}

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(CPP_READY_TIMEOUT, async {
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
            tokio::time::sleep(CPP_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("real C++ daemon at http://{addr} did not become ready within {CPP_READY_TIMEOUT:?}")
    });
}

#[cfg(unix)]
async fn wait_until_ready_mcp_http(url: &str) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(CPP_READY_TIMEOUT, async {
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
            tokio::time::sleep(CPP_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "broker MCP HTTP endpoint at {url} did not become ready within {CPP_READY_TIMEOUT:?}"
        )
    });
}
