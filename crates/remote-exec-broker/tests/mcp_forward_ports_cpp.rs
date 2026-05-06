#![cfg(unix)]

#[path = "support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use remote_exec_broker::client::{Connection, RemoteExecClient};
use remote_exec_proto::public::{
    ExecCommandInput, ForwardPortProtocol, ForwardPortsInput, WriteStdinInput,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn broker_forwards_ports_through_real_cpp_daemon_and_handles_port_conflicts() {
    let fixture = CppDaemonBrokerFixture::spawn().await;
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

    let open = fixture
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Open {
                listen_side: "builder-cpp".to_string(),
                connect_side: "local".to_string(),
                forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                    listen_endpoint: "127.0.0.1:0".to_string(),
                    connect_endpoint: echo_addr.to_string(),
                    protocol: ForwardPortProtocol::Tcp,
                }],
            },
        )
        .await
        .unwrap();
    assert!(!open.is_error, "open failed: {}", open.text_output);
    let opened = &open.structured_content["forwards"][0];
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

    let close = fixture
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Close {
                forward_ids: vec![opened_forward_id],
            },
        )
        .await
        .unwrap();
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn broker_prunes_cpp_exec_sessions_when_daemon_limit_is_reached() {
    let fixture = CppDaemonBrokerFixture::spawn_with_daemon_config(
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
    .await;

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
    assert_eq!(
        first_poll.text_output,
        format!("write_stdin failed: Unknown process id {first_session_id}")
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
    let fixture = CppDaemonBrokerFixture::spawn().await;
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
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Open {
                listen_side: "builder-cpp".to_string(),
                connect_side: "local".to_string(),
                forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                    listen_endpoint: "127.0.0.1:0".to_string(),
                    connect_endpoint: echo_addr.to_string(),
                    protocol: ForwardPortProtocol::Udp,
                }],
            },
        )
        .await
        .unwrap();
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

    let close = fixture
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Close {
                forward_ids: vec![forward_id],
            },
        )
        .await
        .unwrap();
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}

#[tokio::test]
async fn real_cpp_daemon_releases_listener_after_broker_crash() {
    let mut fixture = CrashableCppDaemonBrokerFixture::spawn().await;
    let listen_addr = allocate_addr();

    let open = fixture
        .client
        .call_tool(
            "forward_ports",
            &ForwardPortsInput::Open {
                listen_side: "builder-cpp".to_string(),
                connect_side: "local".to_string(),
                forwards: vec![remote_exec_proto::public::ForwardPortSpec {
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
    fixture.kill_broker().await;

    let (reopened_client, reopened_forward_id) = fixture
        .wait_for_public_forward_reopen(&listen_addr.to_string(), Duration::from_secs(10))
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

struct CppDaemonBrokerFixture {
    _tempdir: TempDir,
    client: RemoteExecClient,
    daemon: tokio::process::Child,
}

impl CppDaemonBrokerFixture {
    async fn spawn() -> Self {
        Self::spawn_with_daemon_config("").await
    }

    async fn spawn_with_daemon_config(extra_daemon_config: &str) -> Self {
        ensure_cpp_daemon_built().await;
        remote_exec_broker::install_crypto_provider();

        let tempdir = tempfile::tempdir().unwrap();
        let daemon_binary = stage_cpp_daemon_binary(tempdir.path());
        let broker_config = tempdir.path().join("broker.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        std::fs::create_dir_all(&daemon_workdir).unwrap();

        let daemon_addr = allocate_addr();
        std::fs::write(
            &daemon_config,
            format!(
                "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = {}\ndefault_workdir = {}\n{}",
                daemon_addr.port(),
                daemon_workdir.display(),
                extra_daemon_config
            ),
        )
        .unwrap();

        let mut daemon = tokio::process::Command::new(&daemon_binary);
        daemon.arg(&daemon_config);
        let daemon = spawn_cpp_daemon_process(&mut daemon).await;
        wait_until_ready_http(daemon_addr).await;

        std::fs::write(
            &broker_config,
            format!(
                r#"[targets.builder-cpp]
base_url = "http://{}"
allow_insecure_http = true
expected_daemon_name = "builder-cpp"

[local]
default_workdir = "{}"
pty = "none"
"#,
                daemon_addr,
                tempdir.path().join("local-work").display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(tempdir.path().join("local-work")).unwrap();

        let client = RemoteExecClient::connect(Connection::Config {
            config_path: broker_config,
        })
        .await
        .unwrap();

        Self {
            _tempdir: tempdir,
            client,
            daemon,
        }
    }
}

impl Drop for CppDaemonBrokerFixture {
    fn drop(&mut self) {
        let _ = self.daemon.start_kill();
    }
}

struct CrashableCppDaemonBrokerFixture {
    _tempdir: TempDir,
    broker_config: PathBuf,
    client: RemoteExecClient,
    broker: tokio::process::Child,
    daemon: tokio::process::Child,
}

impl CrashableCppDaemonBrokerFixture {
    async fn spawn() -> Self {
        ensure_cpp_daemon_built().await;
        remote_exec_broker::install_crypto_provider();

        let tempdir = tempfile::tempdir().unwrap();
        let daemon_binary = stage_cpp_daemon_binary(tempdir.path());
        let broker_config = tempdir.path().join("broker-http.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        let daemon_addr = allocate_addr();
        let broker_addr = allocate_addr();

        std::fs::write(
            &daemon_config,
            format!(
                "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = {}\ndefault_workdir = {}\n",
                daemon_addr.port(),
                daemon_workdir.display()
            ),
        )
        .unwrap();

        let mut daemon = tokio::process::Command::new(&daemon_binary);
        daemon.arg(&daemon_config);
        let daemon = spawn_cpp_daemon_process(&mut daemon).await;
        wait_until_ready_http(daemon_addr).await;

        std::fs::write(
            &broker_config,
            format!(
                r#"[targets.builder-cpp]
base_url = "http://{}"
allow_insecure_http = true
expected_daemon_name = "builder-cpp"

[local]
default_workdir = "{}"
pty = "none"

[mcp]
transport = "streamable_http"
listen = "{}"
path = "/mcp"
"#,
                daemon_addr,
                tempdir.path().join("local-work").display(),
                broker_addr
            ),
        )
        .unwrap();
        std::fs::create_dir_all(tempdir.path().join("local-work")).unwrap();

        let mut broker = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        broker.arg(&broker_config);
        broker.kill_on_drop(true);
        let broker = broker.spawn().unwrap();
        let broker_url = format!("http://{broker_addr}/mcp");
        wait_until_ready_mcp_http(&broker_url).await;
        let client = RemoteExecClient::connect(Connection::StreamableHttp { url: broker_url })
            .await
            .unwrap();

        Self {
            _tempdir: tempdir,
            broker_config,
            client,
            broker,
            daemon,
        }
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
                    "C++ daemon listener on {endpoint} was not released after broker crash; last error={}",
                    response.text_output
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl Drop for CrashableCppDaemonBrokerFixture {
    fn drop(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.daemon.start_kill();
    }
}

fn cpp_daemon_binary() -> PathBuf {
    cpp_daemon_dir().join("build/remote-exec-daemon-cpp")
}

fn cpp_daemon_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../remote-exec-daemon-cpp")
}

fn stage_cpp_daemon_binary(tempdir: &Path) -> PathBuf {
    let staged = tempdir.join("remote-exec-daemon-cpp");
    std::fs::copy(cpp_daemon_binary(), &staged).unwrap();
    staged
}

async fn ensure_cpp_daemon_built() {
    static CPP_DAEMON_BUILD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _build_guard = CPP_DAEMON_BUILD_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    if cpp_daemon_binary().exists() {
        return;
    }

    let cpp_daemon_dir = cpp_daemon_dir();
    let status = tokio::process::Command::new("make")
        .arg("all-posix")
        .current_dir(&cpp_daemon_dir)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "failed to build remote-exec-daemon-cpp");
}

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

fn poll_request(session_id: &str) -> WriteStdinInput {
    WriteStdinInput {
        session_id: session_id.to_string(),
        chars: Some(String::new()),
        yield_time_ms: Some(1),
        max_output_tokens: None,
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

fn allocate_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    remote_exec_broker::install_crypto_provider();
    let client = reqwest::Client::builder().build().unwrap();

    for _ in 0..80 {
        if client
            .post(format!("http://{addr}/v1/health"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("real C++ daemon did not become ready");
}

async fn wait_until_ready_mcp_http(url: &str) {
    remote_exec_broker::install_crypto_provider();
    let client = reqwest::Client::builder().build().unwrap();

    for _ in 0..80 {
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("broker MCP HTTP endpoint did not become ready");
}
