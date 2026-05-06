use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
    transport::TokioChildProcess,
};
use tempfile::TempDir;
use tokio::task::JoinHandle;

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

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
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

    pub async fn stop(&mut self) {
        self.client.close().await.unwrap();
    }

    async fn raw_call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let result = self
            .client
            .call_tool(
                CallToolRequestParams::new(name.to_string())
                    .with_arguments(arguments.as_object().unwrap().clone()),
            )
            .await
            .unwrap();

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
        let broker_addr = allocate_addr();
        let config_path = tempdir.path().join("broker-http.toml");
        std::fs::write(
            &config_path,
            format!(
                "{}\n{}\n[mcp]\ntransport = \"streamable_http\"\nlisten = {}\npath = \"/mcp\"\n",
                daemon_a.target_config_fragment(),
                daemon_b.target_config_fragment(),
                toml_string(&broker_addr.to_string()),
            ),
        )
        .unwrap();

        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
        command.arg(&config_path);
        command.kill_on_drop(true);
        let child = command.spawn().unwrap();
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
    pub workdir: PathBuf,
    client: reqwest::Client,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl DaemonFixture {
    pub async fn spawn(target: &str) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let addr = allocate_addr();
        let workdir = tempdir.path().join("workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let client = build_http_client();

        let mut fixture = Self {
            _tempdir: tempdir,
            target: target.to_string(),
            addr,
            workdir,
            client,
            shutdown: None,
            handle: None,
        };
        fixture.start().await;
        fixture
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.start().await;
    }

    pub fn target_config_fragment(&self) -> String {
        format!(
            r#"[targets.{target}]
base_url = {base_url}
allow_insecure_http = true
expected_daemon_name = {expected_daemon_name}
"#,
            target = self.target,
            base_url = toml_string(&format!("http://{}", self.addr)),
            expected_daemon_name = toml_string(&self.target),
        )
    }

    async fn start(&mut self) {
        let config = remote_exec_daemon::config::DaemonConfig {
            target: self.target.clone(),
            listen: self.addr,
            default_workdir: self.workdir.clone(),
            windows_posix_root: None,
            transport: remote_exec_daemon::config::DaemonTransport::Http,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::Auto,
            default_shell: None,
            yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: None,
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown = Some(shutdown_tx);
        self.handle = Some(tokio::spawn(remote_exec_daemon::run_until(
            config,
            async move {
                let _ = shutdown_rx.await;
            },
        )));
        wait_until_ready_http(&self.client, self.addr).await;
    }

    async fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
            wait_for_listener_release(self.addr).await;
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
    }
}

pub async fn write_png(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save(path).unwrap();
}

pub fn allocate_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

async fn wait_until_ready_http(client: &reqwest::Client, addr: std::net::SocketAddr) {
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
    panic!("daemon did not become ready");
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

fn build_http_client() -> reqwest::Client {
    remote_exec_broker::install_crypto_provider();
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap()
}

async fn wait_for_listener_release(addr: std::net::SocketAddr) {
    #[cfg(windows)]
    let retries = 400;
    #[cfg(not(windows))]
    let retries = 80;

    #[cfg(windows)]
    let delay_ms = 50;
    #[cfg(not(windows))]
    let delay_ms = 25;

    for _ in 0..retries {
        if let Ok(listener) = std::net::TcpListener::bind(addr) {
            drop(listener);
            return;
        }
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    panic!("daemon listener was not released");
}

#[cfg(test)]
mod tests {
    use super::{DaemonFixture, build_http_client};
    use std::path::PathBuf;

    #[test]
    fn target_config_fragment_renders_insecure_http_target() {
        let fixture = DaemonFixture {
            _tempdir: tempfile::tempdir().unwrap(),
            target: "builder-a".to_string(),
            addr: "127.0.0.1:9443".parse().unwrap(),
            workdir: PathBuf::from("/tmp/workdir"),
            client: build_http_client(),
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
