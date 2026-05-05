#[path = "support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use remote_exec_broker::client::{Connection, RemoteExecClient};
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortsInput};
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

struct CppDaemonBrokerFixture {
    _tempdir: TempDir,
    client: RemoteExecClient,
    daemon: tokio::process::Child,
}

impl CppDaemonBrokerFixture {
    async fn spawn() -> Self {
        ensure_cpp_daemon_built().await;

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

fn cpp_daemon_binary() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../remote-exec-daemon-cpp/build/remote-exec-daemon-cpp")
}

async fn ensure_cpp_daemon_built() {
    if cpp_daemon_binary().exists() {
        return;
    }

    let status = tokio::process::Command::new("make")
        .arg("-C")
        .arg("crates/remote-exec-daemon-cpp")
        .arg("all-posix")
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
