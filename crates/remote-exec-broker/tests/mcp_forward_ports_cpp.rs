#![cfg(unix)]

#[path = "support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use remote_exec_broker::client::{Connection, RemoteExecClient};
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortsInput};
use remote_exec_proto::rpc::{
    EmptyResponse, PortForwardProtocol, PortListenCloseRequest, PortListenRequest,
    PortListenResponse,
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

    let rebound = fixture
        .wait_for_daemon_listener_rebind(&listen_addr.to_string(), Duration::from_secs(10))
        .await;
    fixture.close_daemon_bind(&rebound.bind_id).await;

    let reopened_client = RemoteExecClient::connect(Connection::Config {
        config_path: fixture.broker_config.clone(),
    })
    .await
    .unwrap();
    let reopened = reopened_client
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
    assert!(
        !reopened.is_error,
        "reopen failed: {}",
        reopened.text_output
    );
    assert_eq!(
        reopened.structured_content["forwards"][0]["listen_endpoint"],
        listen_addr.to_string()
    );
    let reopened_forward_id = reopened.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();

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
        ensure_cpp_daemon_built().await;
        remote_exec_broker::install_crypto_provider();

        let tempdir = tempfile::tempdir().unwrap();
        let broker_config = tempdir.path().join("broker.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        std::fs::create_dir_all(&daemon_workdir).unwrap();

        let daemon_addr = allocate_addr();
        std::fs::write(
            &daemon_config,
            format!(
                "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = {}\ndefault_workdir = {}\n",
                daemon_addr.port(),
                daemon_workdir.display()
            ),
        )
        .unwrap();

        let daemon = tokio::process::Command::new(cpp_daemon_binary())
            .arg(&daemon_config)
            .spawn()
            .unwrap();
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
    daemon_addr: std::net::SocketAddr,
    http_client: reqwest::Client,
}

impl CrashableCppDaemonBrokerFixture {
    async fn spawn() -> Self {
        ensure_cpp_daemon_built().await;
        remote_exec_broker::install_crypto_provider();

        let tempdir = tempfile::tempdir().unwrap();
        let broker_config = tempdir.path().join("broker-http.toml");
        let daemon_config = tempdir.path().join("daemon-cpp.ini");
        let daemon_workdir = tempdir.path().join("daemon-workdir");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        let daemon_addr = allocate_addr();
        let broker_addr = allocate_addr();
        let http_client = reqwest::Client::builder().build().unwrap();

        std::fs::write(
            &daemon_config,
            format!(
                "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = {}\ndefault_workdir = {}\n",
                daemon_addr.port(),
                daemon_workdir.display()
            ),
        )
        .unwrap();

        let daemon = tokio::process::Command::new(cpp_daemon_binary())
            .arg(&daemon_config)
            .spawn()
            .unwrap();
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
            daemon_addr,
            http_client,
        }
    }

    async fn kill_broker(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.broker.wait().await;
    }

    async fn wait_for_daemon_listener_rebind(
        &self,
        endpoint: &str,
        timeout: Duration,
    ) -> PortListenResponse {
        let started = std::time::Instant::now();
        loop {
            let response = self
                .http_client
                .post(format!("http://{}/v1/port/listen", self.daemon_addr))
                .json(&PortListenRequest {
                    endpoint: endpoint.to_string(),
                    protocol: PortForwardProtocol::Tcp,
                    lease: None,
                })
                .send()
                .await
                .unwrap();
            if response.status().is_success() {
                return response.json::<PortListenResponse>().await.unwrap();
            }
            if started.elapsed() >= timeout {
                panic!(
                    "C++ daemon listener on {endpoint} was not released after broker crash; last status={}",
                    response.status()
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn close_daemon_bind(&self, bind_id: &str) {
        self.http_client
            .post(format!("http://{}/v1/port/listen/close", self.daemon_addr))
            .json(&PortListenCloseRequest {
                bind_id: bind_id.to_string(),
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<EmptyResponse>()
            .await
            .unwrap();
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

async fn ensure_cpp_daemon_built() {
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
