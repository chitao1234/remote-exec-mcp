use std::path::{Path, PathBuf};
use std::time::Duration;

use remote_exec_test_support::test_helpers;
use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;
use rmcp::{RoleClient, ServiceExt, model::CallToolRequestParams, service::RunningService};
use tempfile::TempDir;
use tokio::task::JoinHandle;

#[path = "../support/mod.rs"]
mod shared;
use shared::blocking_child_process::BlockingChildProcess;
use shared::fixture::mcp_tool_name;
use shared::tunnel_drop_proxy::TunnelDropProxy;
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
    let daemon_a = DaemonFixture::spawn(DEFAULT_TEST_TARGET).await;
    let daemon_b = DaemonFixture::spawn("builder-b").await;
    let broker = BrokerFixture::spawn(&daemon_a, &daemon_b).await;

    ClusterFixture {
        broker,
        daemon_a,
        daemon_b,
    }
}

fn apply_quiet_test_logging(command: &mut tokio::process::Command) {
    shared::apply_quiet_test_logging(command);
}

pub fn long_running_tty_exec_input(target: &str) -> serde_json::Value {
    #[cfg(windows)]
    let cmd = "echo hello & ping -n 30 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let cmd = "printf hello; read line";

    #[cfg(windows)]
    let mut arguments = serde_json::json!({
        "target": target,
        "cmd": cmd,
        "login": false,
        "tty": true,
        "yield_time_ms": 250,
    });
    #[cfg(not(windows))]
    let arguments = serde_json::json!({
        "target": target,
        "cmd": cmd,
        "login": false,
        "shell": "/bin/sh",
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

pub use shared::assert_correlated_tool_error;
pub use shared::assert_stream_closed;

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
        let client = serve_broker_child_stdio(command).await;

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
        let mcp_name = mcp_tool_name(name);
        let params = CallToolRequestParams::new(mcp_name)
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

async fn serve_broker_child_stdio(
    command: tokio::process::Command,
) -> RunningService<RoleClient, DummyClientHandler> {
    let transport = BlockingChildProcess::spawn(command).unwrap();
    DummyClientHandler.serve(transport).await.unwrap()
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
        shared::streamable_http_child::configure_streamable_http_broker_child(&mut command);
        command.kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let broker_addr = shared::streamable_http_child::wait_for_streamable_http_bound_addr(
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

pub use shared::fixture::{DummyClientHandler, ToolResult};

pub struct DaemonFixture {
    _tempdir: TempDir,
    pub target: String,
    pub addr: std::net::SocketAddr,
    backend_addr: std::net::SocketAddr,
    pub workdir: PathBuf,
    proxy: TunnelDropProxy,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl DaemonFixture {
    pub async fn spawn(target: &str) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let backend_listener =
            bind_reusable_daemon_test_listener("127.0.0.1:0".parse().unwrap()).unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
        let workdir = tempdir.path().join("workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let proxy = TunnelDropProxy::spawn(
            backend_addr,
            shared::tunnel_drop_proxy::TunnelDropProxyOptions {
                panic_context: "multi-target tunnel-drop proxy",
                ..Default::default()
            },
        )
        .await;
        let addr = proxy.listen_addr;

        let mut fixture = Self {
            _tempdir: tempdir,
            target: target.to_string(),
            addr,
            backend_addr,
            workdir,
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
            connection_mode: remote_exec_daemon::config::DaemonConnectionMode::Listen,
            reverse: None,
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
            request_timeout_ms: 300_000,
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
        wait_until_ready_http(self.addr).await;
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

pub async fn write_png(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save(path).unwrap();
}

pub use shared::{spawn_tcp_echo, spawn_udp_echo};

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    shared::wait_until_ready_http(
        addr,
        MULTI_TARGET_READY_TIMEOUT,
        MULTI_TARGET_READY_POLL,
        "daemon HTTP endpoint",
    )
    .await;
}

async fn wait_until_ready_mcp_http(url: &str) {
    shared::wait_until_ready_mcp_http(
        url,
        MULTI_TARGET_READY_TIMEOUT,
        MULTI_TARGET_READY_POLL,
        "streamable HTTP broker",
    )
    .await;
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

    use super::DEFAULT_TEST_TARGET;
    use super::{DaemonFixture, TunnelDropProxy};

    #[test]
    fn target_config_fragment_renders_insecure_http_target() {
        let fixture = DaemonFixture {
            _tempdir: tempfile::tempdir().unwrap(),
            target: DEFAULT_TEST_TARGET.to_string(),
            addr: "127.0.0.1:9443".parse().unwrap(),
            backend_addr: "127.0.0.1:9444".parse().unwrap(),
            workdir: PathBuf::from("/tmp/workdir"),
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
            parsed["targets"][DEFAULT_TEST_TARGET]["base_url"].as_str(),
            Some("http://127.0.0.1:9443")
        );
        assert_eq!(
            parsed["targets"][DEFAULT_TEST_TARGET]["allow_insecure_http"].as_bool(),
            Some(true)
        );
    }
}
