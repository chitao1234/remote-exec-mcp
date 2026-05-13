use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use remote_exec_proto::port_tunnel::{read_frame, write_frame};
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
    transport::TokioChildProcess,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

#[path = "../support/streamable_http_child.rs"]
mod streamable_http_child;
#[path = "../../../../tests/support/test_helpers.rs"]
mod test_helpers;

const BROKER_TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const BROKER_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const MULTI_TARGET_READY_TIMEOUT: Duration = Duration::from_secs(20);
const MULTI_TARGET_READY_POLL: Duration = Duration::from_millis(50);

pub struct ClusterFixture {
    pub broker: BrokerFixture,
    pub daemon_a: DaemonFixture,
    pub daemon_b: DaemonFixture,
}

pub async fn spawn_cluster() -> ClusterFixture {
    let daemon_a = DaemonFixture::spawn("builder-a").await;
    let daemon_b = DaemonFixture::spawn("builder-b").await;
    let broker = BrokerFixture::spawn(&daemon_a, &daemon_b).await;

    ClusterFixture {
        broker,
        daemon_a,
        daemon_b,
    }
}

fn apply_quiet_test_logging(command: &mut tokio::process::Command) {
    if std::env::var_os("REMOTE_EXEC_LOG").is_some() || std::env::var_os("RUST_LOG").is_some() {
        return;
    }

    let filter = std::env::var("REMOTE_EXEC_TEST_LOG").unwrap_or_else(|_| "error".to_string());
    command.env("REMOTE_EXEC_LOG", filter);
}

pub fn long_running_tty_exec_input(target: &str) -> serde_json::Value {
    #[cfg(windows)]
    let cmd = "echo hello & ping -n 30 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let cmd = "printf hello; sleep 30";

    #[cfg(windows)]
    let mut arguments = serde_json::json!({
        "target": target,
        "cmd": cmd,
        "tty": true,
        "yield_time_ms": 250,
    });
    #[cfg(not(windows))]
    let arguments = serde_json::json!({
        "target": target,
        "cmd": cmd,
        "tty": true,
        "yield_time_ms": 250,
    });

    #[cfg(windows)]
    {
        arguments.as_object_mut().unwrap().insert(
            "shell".to_string(),
            serde_json::Value::String("cmd.exe".to_string()),
        );
    }

    arguments
}

pub fn assert_correlated_tool_error(
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

pub struct BrokerFixture {
    pub _tempdir: TempDir,
    pub client: RunningService<RoleClient, DummyClientHandler>,
}

impl BrokerFixture {
    pub async fn spawn(daemon_a: &DaemonFixture, daemon_b: &DaemonFixture) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join("broker.toml");
        std::fs::write(
            &config_path,
            format!(
                "{}\n{}",
                daemon_a.target_config_fragment(),
                daemon_b.target_config_fragment(),
            ),
        )
        .unwrap();

        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        command.arg(&config_path);
        apply_quiet_test_logging(&mut command);
        let transport = TokioChildProcess::new(command).unwrap();
        let client = DummyClientHandler.serve(transport).await.unwrap();

        Self {
            _tempdir: tempdir,
            client,
        }
    }

    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let result = self.raw_call_tool(name, arguments).await;
        assert!(
            !result.is_error,
            "expected successful tool call, got {}",
            result.text_output
        );
        result
    }

    pub async fn call_tool_error(&self, name: &str, arguments: serde_json::Value) -> String {
        let result = self.raw_call_tool(name, arguments).await;
        assert!(
            result.is_error,
            "expected tool error, text={}, structured={}, raw={}",
            result.text_output,
            result.structured_content,
            serde_json::Value::Array(result.raw_content.clone())
        );
        result.text_output
    }

    pub async fn open_tcp_forward(
        &self,
        listen_side: &str,
        connect_side: &str,
        listen_endpoint: &str,
        connect_endpoint: &str,
    ) -> ToolResult {
        self.call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": listen_side,
                "connect_side": connect_side,
                "forwards": [{
                    "listen_endpoint": listen_endpoint,
                    "connect_endpoint": connect_endpoint,
                    "protocol": "tcp"
                }]
            }),
        )
        .await
    }

    pub async fn open_udp_forward(
        &self,
        listen_side: &str,
        connect_side: &str,
        listen_endpoint: &str,
        connect_endpoint: &str,
    ) -> ToolResult {
        self.call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": listen_side,
                "connect_side": connect_side,
                "forwards": [{
                    "listen_endpoint": listen_endpoint,
                    "connect_endpoint": connect_endpoint,
                    "protocol": "udp"
                }]
            }),
        )
        .await
    }

    pub async fn stop(&mut self) {
        let closed = self
            .client
            .close_with_timeout(BROKER_CLOSE_TIMEOUT)
            .await
            .unwrap();
        assert!(
            closed.is_some(),
            "broker MCP child did not close within {BROKER_CLOSE_TIMEOUT:?}"
        );
    }

    async fn raw_call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let params = CallToolRequestParams::new(name.to_string())
            .with_arguments(arguments.as_object().unwrap().clone());
        let result = tokio::time::timeout(BROKER_TOOL_CALL_TIMEOUT, self.client.call_tool(params))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "tool call `{name}` timed out after {BROKER_TOOL_CALL_TIMEOUT:?}; arguments={arguments}"
                )
            })
            .unwrap_or_else(|err| panic!("tool call `{name}` failed: {err}; arguments={arguments}"));

        ToolResult::from_call_tool_result(result)
    }
}

pub struct HttpBrokerFixture {
    _tempdir: TempDir,
    pub url: String,
    child: tokio::process::Child,
}

impl HttpBrokerFixture {
    pub async fn spawn(daemon_a: &DaemonFixture, daemon_b: &DaemonFixture) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join("broker-http.toml");
        std::fs::write(
            &config_path,
            format!(
                "{}\n{}\n[mcp]\ntransport = \"streamable_http\"\nlisten = {}\npath = \"/mcp\"\n",
                daemon_a.target_config_fragment(),
                daemon_b.target_config_fragment(),
                test_helpers::toml_string("127.0.0.1:0"),
            ),
        )
        .unwrap();

        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        command.arg(&config_path);
        apply_quiet_test_logging(&mut command);
        streamable_http_child::configure_streamable_http_broker_child(&mut command);
        command.kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let broker_addr = streamable_http_child::wait_for_streamable_http_bound_addr(
            &mut child,
            "multi-target broker",
        )
        .await;
        let url = format!("http://{broker_addr}/mcp");
        wait_until_ready_mcp_http(&url).await;

        Self {
            _tempdir: tempdir,
            url,
            child,
        }
    }

    pub async fn kill(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

impl Drop for HttpBrokerFixture {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub struct ToolResult {
    pub is_error: bool,
    pub text_output: String,
    pub structured_content: serde_json::Value,
    pub raw_content: Vec<serde_json::Value>,
}

impl ToolResult {
    fn from_call_tool_result(result: CallToolResult) -> Self {
        let text_output = result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let raw_content = result.content.iter().map(normalize_content).collect();

        Self {
            is_error: result.is_error.unwrap_or(false),
            text_output,
            structured_content: result.structured_content.unwrap_or(serde_json::Value::Null),
            raw_content,
        }
    }

    pub fn forward_id(&self) -> String {
        self.structured_content["forwards"][0]["forward_id"]
            .as_str()
            .expect("forward result should include forward_id")
            .to_string()
    }

    pub fn listen_endpoint(&self) -> String {
        self.structured_content["forwards"][0]["listen_endpoint"]
            .as_str()
            .expect("forward result should include listen_endpoint")
            .to_string()
    }
}

fn normalize_content(content: &rmcp::model::Content) -> serde_json::Value {
    if let Some(text) = content.raw.as_text() {
        return serde_json::json!({
            "type": "text",
            "text": text.text,
        });
    }

    if let Some(image) = content.raw.as_image() {
        return serde_json::json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        });
    }

    serde_json::to_value(content).unwrap()
}

#[derive(Debug, Clone, Default)]
pub struct DummyClientHandler;

impl ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

pub struct DaemonFixture {
    _tempdir: TempDir,
    pub target: String,
    pub addr: std::net::SocketAddr,
    backend_addr: std::net::SocketAddr,
    pub workdir: PathBuf,
    client: reqwest::Client,
    proxy: TunnelDropProxy,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<JoinHandle<anyhow::Result<()>>>,
}

struct TunnelDropProxy {
    listen_addr: std::net::SocketAddr,
    daemon_addr: Arc<Mutex<std::net::SocketAddr>>,
    active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<PortTunnelAction>>>>,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

enum PortTunnelAction {
    Drop,
    Corrupt,
}

impl DaemonFixture {
    pub async fn spawn(target: &str) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let backend_listener =
            bind_reusable_daemon_test_listener("127.0.0.1:0".parse().unwrap()).unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
        let workdir = tempdir.path().join("workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let client = build_http_client();
        let proxy = TunnelDropProxy::spawn(backend_addr).await;
        let addr = proxy.listen_addr;

        let mut fixture = Self {
            _tempdir: tempdir,
            target: target.to_string(),
            addr,
            backend_addr,
            workdir,
            client,
            proxy,
            shutdown: None,
            handle: None,
        };
        fixture.start_on_listener(backend_listener).await;
        fixture
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.start().await;
    }

    pub async fn drop_port_tunnels(&self) {
        self.proxy.drop_port_tunnels().await;
        self.proxy.assert_no_task_panics().await;
    }

    pub async fn corrupt_port_tunnels(&self) {
        self.proxy.corrupt_port_tunnels().await;
        self.proxy.assert_no_task_panics().await;
    }

    pub fn target_config_fragment(&self) -> String {
        format!(
            r#"[targets.{target}]
base_url = {base_url}
allow_insecure_http = true
expected_daemon_name = {expected_daemon_name}
"#,
            target = self.target,
            base_url = test_helpers::toml_string(&format!("http://{}", self.addr)),
            expected_daemon_name = test_helpers::toml_string(&self.target),
        )
    }

    fn daemon_config(&self) -> remote_exec_daemon::config::DaemonConfig {
        remote_exec_daemon::config::DaemonConfig {
            target: self.target.clone(),
            listen: self.backend_addr,
            default_workdir: self.workdir.clone(),
            windows_posix_root: None,
            transport: remote_exec_daemon::config::DaemonTransport::Http,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
            max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::Auto,
            default_shell: None,
            yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
            port_forward_limits: remote_exec_daemon::config::HostPortForwardLimits::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: None,
        }
    }

    async fn start_on_listener(&mut self, listener: tokio::net::TcpListener) {
        let config = self.daemon_config();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown = Some(shutdown_tx);
        self.handle = Some(tokio::spawn(
            remote_exec_daemon::test_support::run_until_on_listener(config, listener, async move {
                let _ = shutdown_rx.await;
            }),
        ));
        wait_until_ready_http(&self.client, self.addr).await;
    }

    async fn start(&mut self) {
        let listener = bind_reusable_daemon_test_listener("127.0.0.1:0".parse().unwrap())
            .expect("bind daemon backend listener");
        self.backend_addr = listener
            .local_addr()
            .expect("read daemon backend listener addr");
        self.proxy.set_daemon_addr(self.backend_addr).await;
        self.start_on_listener(listener).await;
    }

    async fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        self.proxy.stop();
    }
}

impl TunnelDropProxy {
    async fn spawn(daemon_addr: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind tunnel drop proxy");
        let listen_addr = listener.local_addr().expect("read tunnel drop proxy addr");
        let daemon_addr = Arc::new(Mutex::new(daemon_addr));
        let active_port_tunnels = Arc::new(Mutex::new(Vec::new()));
        let background_tasks = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let daemon_addr_task = daemon_addr.clone();
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
                        let daemon_addr = daemon_addr_task.clone();
                        let active_port_tunnels = active_port_tunnels_task.clone();
                        let connection_handle = tokio::spawn(async move {
                            let daemon_addr = *daemon_addr.lock().await;
                            if let Err(err) = proxy_connection(stream, daemon_addr, active_port_tunnels).await {
                                if is_expected_proxy_teardown_error(&err) {
                                    return;
                                }
                                panic!("multi-target tunnel-drop proxy connection failed: {err}");
                            }
                        });
                        background_tasks_accept.lock().await.push(connection_handle);
                    }
                }
            }
        });

        Self {
            listen_addr,
            daemon_addr,
            active_port_tunnels,
            background_tasks,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    async fn set_daemon_addr(&self, addr: std::net::SocketAddr) {
        *self.daemon_addr.lock().await = addr;
    }

    async fn drop_port_tunnels(&self) {
        let mut active = self.active_port_tunnels.lock().await;
        for shutdown in active.drain(..) {
            let _ = shutdown.send(PortTunnelAction::Drop);
        }
    }

    async fn corrupt_port_tunnels(&self) {
        let mut active = self.active_port_tunnels.lock().await;
        for shutdown in active.drain(..) {
            let _ = shutdown.send(PortTunnelAction::Corrupt);
        }
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
            handle
                .await
                .expect("multi-target tunnel-drop proxy task panicked");
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
    active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<PortTunnelAction>>>>,
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

    if is_port_tunnel {
        backend_stream.write_all(&request).await?;
        let (drop_tx, drop_rx) = oneshot::channel();
        active_port_tunnels.lock().await.push(drop_tx);
        proxy_port_tunnel_streams(client_stream, backend_stream, drop_rx).await
    } else {
        let request = rewrite_request_connection_close(&request_text);
        backend_stream.write_all(&request).await?;
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

fn rewrite_request_connection_close(request: &str) -> Vec<u8> {
    let (headers, _) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut lines = headers.lines();
    let mut rewritten = String::new();
    if let Some(first_line) = lines.next() {
        rewritten.push_str(first_line);
        rewritten.push_str("\r\n");
    }
    for line in lines {
        if line.to_ascii_lowercase().starts_with("connection:") {
            continue;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("Connection: close\r\n\r\n");
    rewritten.into_bytes()
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
    mut drop_rx: oneshot::Receiver<PortTunnelAction>,
) -> std::io::Result<()> {
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream) => {
            let _ = result?;
        }
        action = &mut drop_rx => {
            match action {
                Ok(PortTunnelAction::Drop) | Err(_) => {
                    let _ = client_stream.shutdown().await;
                    let _ = backend_stream.shutdown().await;
                }
                Ok(PortTunnelAction::Corrupt) => {
                    backend_stream
                        .write_all(&[
                            remote_exec_proto::port_tunnel::FrameType::TcpData as u8,
                            0,
                            1,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                        ])
                        .await?;
                    if let Ok(Ok(frame)) = tokio::time::timeout(
                        Duration::from_secs(1),
                        read_frame(&mut backend_stream),
                    )
                    .await
                    {
                        write_frame(&mut client_stream, &frame).await?;
                    }
                    let _ = backend_stream.shutdown().await;
                    let _ = client_stream.shutdown().await;
                }
            }
        }
    }

    Ok(())
}

pub async fn write_png(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save(path).unwrap();
}

pub async fn spawn_tcp_echo() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
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
    addr
}

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

async fn wait_until_ready_http(client: &reqwest::Client, addr: std::net::SocketAddr) {
    tokio::time::timeout(MULTI_TARGET_READY_TIMEOUT, async {
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
            tokio::time::sleep(MULTI_TARGET_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "daemon HTTP endpoint at http://{addr} did not become ready within {MULTI_TARGET_READY_TIMEOUT:?}"
        )
    });
}

async fn wait_until_ready_mcp_http(url: &str) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(MULTI_TARGET_READY_TIMEOUT, async {
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
            tokio::time::sleep(MULTI_TARGET_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "streamable HTTP broker at {url} did not become ready within {MULTI_TARGET_READY_TIMEOUT:?}"
        )
    });
}

fn build_http_client() -> reqwest::Client {
    remote_exec_broker::install_crypto_provider().unwrap();
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap()
}

fn bind_reusable_daemon_test_listener(
    addr: std::net::SocketAddr,
) -> std::io::Result<tokio::net::TcpListener> {
    let socket = if addr.is_ipv4() {
        tokio::net::TcpSocket::new_v4()?
    } else {
        tokio::net::TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
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

pub async fn wait_for_forward_status_timeout(
    broker: &BrokerFixture,
    forward_id: &str,
    status: &str,
    timeout: Duration,
) -> Option<serde_json::Value> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        let list = broker
            .call_tool(
                "forward_ports",
                serde_json::json!({
                    "action": "list",
                    "forward_ids": [forward_id]
                }),
            )
            .await;
        let entry = list.structured_content["forwards"][0].clone();
        if entry["status"] == status {
            return Some(entry);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    None
}

pub async fn wait_for_forward_ready_after_reconnect(
    broker: &BrokerFixture,
    forward_id: &str,
    timeout: Duration,
) -> serde_json::Value {
    let started = std::time::Instant::now();
    let mut last_entry = serde_json::Value::Null;
    while started.elapsed() < timeout {
        let list = broker
            .call_tool(
                "forward_ports",
                serde_json::json!({
                    "action": "list",
                    "forward_ids": [forward_id]
                }),
            )
            .await;
        let entry = list.structured_content["forwards"][0].clone();
        if entry["phase"] == "ready" && entry["reconnect_attempts"].as_u64().unwrap_or_default() > 0
        {
            return entry;
        }
        last_entry = entry;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "forward `{}` did not return to ready after reconnect within {:?}; last_status={} last_phase={} reconnect_attempts={} last_error={}",
        forward_id,
        timeout,
        last_entry["status"].as_str().unwrap_or("<missing>"),
        last_entry["phase"].as_str().unwrap_or("<missing>"),
        last_entry["reconnect_attempts"]
            .as_u64()
            .unwrap_or_default(),
        last_entry["last_error"].as_str().unwrap_or("<none>")
    );
}

pub async fn wait_for_daemon_listener_rebind(endpoint: &str, timeout: Duration) {
    const DAEMON_LISTENER_REBIND_POLL: Duration = Duration::from_millis(200);

    let started = std::time::Instant::now();
    loop {
        if tokio::net::TcpListener::bind(endpoint).await.is_ok() {
            return;
        }
        if started.elapsed() >= timeout {
            panic!("daemon listener on {endpoint} was not released within {timeout:?}");
        }
        tokio::time::sleep(DAEMON_LISTENER_REBIND_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::{DaemonFixture, TunnelDropProxy, build_http_client};

    #[test]
    fn target_config_fragment_renders_insecure_http_target() {
        let fixture = DaemonFixture {
            _tempdir: tempfile::tempdir().unwrap(),
            target: "builder-a".to_string(),
            addr: "127.0.0.1:9443".parse().unwrap(),
            backend_addr: "127.0.0.1:9444".parse().unwrap(),
            workdir: PathBuf::from("/tmp/workdir"),
            client: build_http_client(),
            proxy: TunnelDropProxy {
                listen_addr: "127.0.0.1:9443".parse().unwrap(),
                daemon_addr: Arc::new(Mutex::new("127.0.0.1:9444".parse().unwrap())),
                active_port_tunnels: Arc::new(Mutex::new(Vec::new())),
                background_tasks: Arc::new(Mutex::new(Vec::new())),
                shutdown: None,
                handle: None,
            },
            shutdown: None,
            handle: None,
        };

        let parsed = fixture
            .target_config_fragment()
            .parse::<toml::Table>()
            .expect("config fragment should parse as TOML");

        assert_eq!(
            parsed["targets"]["builder-a"]["base_url"].as_str(),
            Some("http://127.0.0.1:9443")
        );
        assert_eq!(
            parsed["targets"]["builder-a"]["allow_insecure_http"].as_bool(),
            Some(true)
        );
    }
}
