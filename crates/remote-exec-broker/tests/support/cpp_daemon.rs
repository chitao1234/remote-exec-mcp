use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::Duration;

use remote_exec_broker::{Connection, RemoteExecClient, ToolResponse};
use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::public::{ForwardPortProtocol, ForwardPortsInput};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};

pub const CPP_TARGET: &str = "builder-cpp";

const CPP_READY_TIMEOUT: Duration = Duration::from_secs(20);
const CPP_READY_POLL: Duration = Duration::from_millis(50);
static MISSING_CPP_DAEMON_WARNING: Once = Once::new();

pub struct CppDaemonBrokerFixture {
    _tempdir: TempDir,
    pub client: RemoteExecClient,
    proxy: TunnelDropProxy,
    daemon: tokio::process::Child,
    daemon_workdir: PathBuf,
    local_workdir: PathBuf,
}

impl CppDaemonBrokerFixture {
    pub async fn spawn() -> Option<Self> {
        Self::spawn_with_daemon_config("").await
    }

    pub async fn spawn_with_daemon_config(extra_daemon_config: &str) -> Option<Self> {
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
        let local_workdir = tempdir.path().join("local-work");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        std::fs::create_dir_all(&local_workdir).unwrap();

        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        let daemon_config_body = format!(
            "target = {CPP_TARGET}\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n{}",
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
                r#"[targets.{target}]
base_url = "http://{daemon_addr}"
allow_insecure_http = true
expected_daemon_name = "{target}"

[local]
default_workdir = {local_workdir}
pty = "none"
"#,
                target = CPP_TARGET,
                daemon_addr = daemon_addr,
                local_workdir =
                    super::test_helpers::toml_string(&local_workdir.display().to_string()),
            ),
        )
        .unwrap();

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
            daemon_workdir,
            local_workdir,
        })
    }

    pub fn daemon_workdir(&self) -> &Path {
        &self.daemon_workdir
    }

    pub fn local_workdir(&self) -> &Path {
        &self.local_workdir
    }

    pub async fn open_forward(
        &self,
        listen_side: &str,
        connect_side: &str,
        listen_endpoint: String,
        connect_endpoint: String,
        protocol: ForwardPortProtocol,
    ) -> ToolResponse {
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

    pub async fn open_tcp_forward(&self, connect_endpoint: &str) -> ToolResponse {
        self.open_forward(
            CPP_TARGET,
            "local",
            "127.0.0.1:0".to_string(),
            connect_endpoint.to_string(),
            ForwardPortProtocol::Tcp,
        )
        .await
    }

    pub async fn open_tcp_forward_local_to_cpp(&self, connect_endpoint: &str) -> ToolResponse {
        self.open_forward(
            "local",
            CPP_TARGET,
            "127.0.0.1:0".to_string(),
            connect_endpoint.to_string(),
            ForwardPortProtocol::Tcp,
        )
        .await
    }

    pub async fn close_forward(&self, forward_id: String) -> ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Close {
                    forward_ids: vec![forward_id.into()],
                },
            )
            .await
            .unwrap()
    }

    pub async fn drop_port_tunnels(&self) {
        self.proxy.drop_port_tunnels().await;
    }
}

impl Drop for CppDaemonBrokerFixture {
    fn drop(&mut self) {
        self.proxy.stop();
        let _ = self.daemon.start_kill();
    }
}

pub async fn wait_for_forward_ready(
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
                    forward_ids: vec![ForwardId::new(forward_id)],
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

pub struct CrashableCppDaemonBrokerFixture {
    _tempdir: TempDir,
    broker_config: PathBuf,
    pub client: RemoteExecClient,
    broker: tokio::process::Child,
    daemon: tokio::process::Child,
}

impl CrashableCppDaemonBrokerFixture {
    pub async fn spawn() -> Option<Self> {
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
        let local_workdir = tempdir.path().join("local-work");
        std::fs::create_dir_all(&daemon_workdir).unwrap();
        std::fs::create_dir_all(&local_workdir).unwrap();
        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        let daemon_config_body = format!(
            "target = {CPP_TARGET}\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n",
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
                r#"[targets.{target}]
base_url = "http://{daemon_addr}"
allow_insecure_http = true
expected_daemon_name = "{target}"

[local]
default_workdir = {local_workdir}
pty = "none"

[mcp]
transport = "streamable_http"
listen = {broker_listen}
path = "/mcp"
"#,
                target = CPP_TARGET,
                daemon_addr = daemon_addr,
                local_workdir =
                    super::test_helpers::toml_string(&local_workdir.display().to_string()),
                broker_listen = super::test_helpers::toml_string("127.0.0.1:0"),
            ),
        )
        .unwrap();

        let mut broker = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        broker.arg(&broker_config);
        apply_quiet_test_logging(&mut broker);
        super::streamable_http_child::configure_streamable_http_broker_child(&mut broker);
        broker.kill_on_drop(true);
        let mut broker = broker.spawn().unwrap();
        let broker_addr = super::streamable_http_child::wait_for_streamable_http_bound_addr(
            &mut broker,
            "C++ broker",
        )
        .await;
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

    pub async fn kill_broker(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.broker.wait().await;
    }

    pub async fn wait_for_public_forward_reopen(
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
                        listen_side: CPP_TARGET.to_string(),
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

impl Drop for CrashableCppDaemonBrokerFixture {
    fn drop(&mut self) {
        let _ = self.broker.start_kill();
        let _ = self.daemon.start_kill();
    }
}

struct TunnelDropProxy {
    listen_addr: std::net::SocketAddr,
    active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
    background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
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

    async fn drop_port_tunnels(&self) {
        let mut active = self.active_port_tunnels.lock().await;
        for shutdown in active.drain(..) {
            let _ = shutdown.send(());
        }
        drop(active);
        self.assert_no_task_panics().await;
    }

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

    cpp_daemon_default_binaries()
        .into_iter()
        .find(|path| path.exists())
}

fn cpp_daemon_default_binaries() -> Vec<PathBuf> {
    let daemon_dir = cpp_daemon_dir();
    if cfg!(windows) {
        vec![
            daemon_dir.join("build/remote-exec-daemon-cpp-msvc.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-native-xp-ws2.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-native-2000-ws2.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-native-nt4-ws1.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp-native.exe"),
            daemon_dir.join("build/remote-exec-daemon-cpp.exe"),
        ]
    } else {
        vec![daemon_dir.join("build/remote-exec-daemon-cpp")]
    }
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

fn cpp_daemon_skip_message() -> String {
    let default = cpp_daemon_default_binaries()
        .into_iter()
        .next()
        .expect("at least one default C++ daemon path");
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
        let default_paths = cpp_daemon_default_binaries()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
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
!!! default checked paths    = {default_paths}\n\
!!!                                                                        !!!\n\
!!! {skip_message}\n\
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n",
            default_paths = default_paths,
            skip_message = cpp_daemon_skip_message()
        );
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(message.as_bytes());
        let _ = stderr.flush();
    });
}
