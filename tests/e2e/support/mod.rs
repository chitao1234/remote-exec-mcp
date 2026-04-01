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

    async fn raw_call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let result = self
            .client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: name.to_string().into(),
                arguments: Some(arguments.as_object().unwrap().clone()),
                task: None,
            })
            .await
            .unwrap();

        ToolResult::from_call_tool_result(result)
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
    ca_pem: PathBuf,
    client_cert_pem: PathBuf,
    client_key_pem: PathBuf,
    daemon_cert_pem: PathBuf,
    daemon_key_pem: PathBuf,
    client: reqwest::Client,
    handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl DaemonFixture {
    pub async fn spawn(target: &str) -> Self {
        remote_exec_daemon::install_crypto_provider();

        let tempdir = tempfile::tempdir().unwrap();
        let certs = write_test_certs(tempdir.path());
        let addr = allocate_addr();
        let workdir = tempdir.path().join("workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(
                reqwest::Certificate::from_pem(&std::fs::read(&certs.ca_cert).unwrap()).unwrap(),
            )
            .identity(
                reqwest::Identity::from_pem(
                    &[
                        std::fs::read(&certs.client_cert).unwrap(),
                        std::fs::read(&certs.client_key).unwrap(),
                    ]
                    .concat(),
                )
                .unwrap(),
            )
            .build()
            .unwrap();

        let mut fixture = Self {
            _tempdir: tempdir,
            target: target.to_string(),
            addr,
            workdir,
            ca_pem: certs.ca_cert,
            client_cert_pem: certs.client_cert,
            client_key_pem: certs.client_key,
            daemon_cert_pem: certs.daemon_cert,
            daemon_key_pem: certs.daemon_key,
            client,
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
base_url = "https://{addr}"
ca_pem = "{ca_pem}"
client_cert_pem = "{client_cert_pem}"
client_key_pem = "{client_key_pem}"
expected_daemon_name = "{target}"
"#,
            target = self.target,
            addr = self.addr,
            ca_pem = self.ca_pem.display(),
            client_cert_pem = self.client_cert_pem.display(),
            client_key_pem = self.client_key_pem.display(),
        )
    }

    async fn start(&mut self) {
        let config = remote_exec_daemon::config::DaemonConfig {
            target: self.target.clone(),
            listen: self.addr,
            default_workdir: self.workdir.clone(),
            allow_login_shell: true,
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: self.daemon_cert_pem.clone(),
                key_pem: self.daemon_key_pem.clone(),
                ca_pem: self.ca_pem.clone(),
            },
        };
        self.handle = Some(tokio::spawn(remote_exec_daemon::run(config)));
        wait_until_ready(&self.client, self.addr).await;
    }

    async fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
            wait_for_listener_release(self.addr).await;
        }
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

pub async fn write_png(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save(path).unwrap();
}

struct TestCerts {
    ca_cert: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
    daemon_cert: PathBuf,
    daemon_key: PathBuf,
}

fn write_test_certs(dir: &Path) -> TestCerts {
    let out_dir = dir.join("certs");
    let spec = remote_exec_pki::DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![remote_exec_pki::DaemonCertSpec::localhost("builder-a")],
    };
    let bundle = remote_exec_pki::build_dev_init_bundle(&spec).unwrap();
    let manifest = remote_exec_pki::write_dev_init_bundle(&spec, &bundle, &out_dir, true).unwrap();
    let daemon = manifest.daemons.get("builder-a").unwrap();

    TestCerts {
        ca_cert: manifest.ca.cert_pem.clone(),
        client_cert: manifest.broker.cert_pem.clone(),
        client_key: manifest.broker.key_pem.clone(),
        daemon_cert: daemon.cert_pem.clone(),
        daemon_key: daemon.key_pem.clone(),
    }
}

fn allocate_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

async fn wait_until_ready(client: &reqwest::Client, addr: std::net::SocketAddr) {
    for _ in 0..80 {
        if client
            .post(format!("https://{addr}/v1/health"))
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

async fn wait_for_listener_release(addr: std::net::SocketAddr) {
    for _ in 0..80 {
        if let Ok(listener) = std::net::TcpListener::bind(addr) {
            drop(listener);
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon listener was not released");
}
