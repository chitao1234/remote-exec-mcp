#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWarning, ExecWriteRequest, HealthCheckResponse,
    ImageReadRequest, ImageReadResponse, PatchApplyRequest, PatchApplyResponse, RpcErrorBody,
    TargetInfoResponse,
};
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
    transport::TokioChildProcess,
};
use tempfile::TempDir;
use tokio::sync::Mutex;

pub struct BrokerFixture {
    pub _tempdir: TempDir,
    pub client: RunningService<RoleClient, DummyClientHandler>,
    stub_state: StubDaemonState,
}

#[derive(Debug, Clone)]
pub enum StubImageReadResponse {
    Success(ImageReadResponse),
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Debug, Clone, Copy)]
enum ExecWriteBehavior {
    Success,
    TemporaryFailureOnce,
    UnknownSession,
}

#[allow(dead_code)]
pub struct DelayedTargetFixture {
    pub broker: BrokerFixture,
    certs: TestCerts,
    addr: std::net::SocketAddr,
}

#[allow(dead_code)]
impl DelayedTargetFixture {
    pub async fn spawn_target(&self, target: &str) {
        spawn_named_daemon_on_addr(
            &self.certs,
            self.addr,
            stub_daemon_state(target, ExecWriteBehavior::Success),
        )
        .await;
    }
}

impl BrokerFixture {
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let result = self.raw_call_tool(name, arguments).await;
        assert!(
            !result.is_error,
            "expected successful tool call, got {}",
            result.text_output
        );
        result
    }

    #[allow(dead_code)]
    pub async fn raw_tool_result(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        self.raw_call_tool(name, arguments).await
    }

    #[allow(dead_code)]
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

    pub async fn exec_start_calls(&self) -> usize {
        *self.stub_state.exec_start_calls.lock().await
    }

    pub async fn last_patch_request(&self) -> Option<PatchApplyRequest> {
        self.stub_state.last_patch_request.lock().await.clone()
    }

    pub async fn set_image_read_response(&self, response: StubImageReadResponse) {
        *self.stub_state.image_read_response.lock().await = response;
    }

    pub async fn set_exec_start_warnings(&self, warnings: Vec<ExecWarning>) {
        *self.stub_state.exec_start_warnings.lock().await = warnings;
    }
}

#[allow(dead_code)]
pub struct ToolResult {
    pub is_error: bool,
    pub text_output: String,
    pub structured_content: serde_json::Value,
    pub raw_content: Vec<serde_json::Value>,
    pub meta: Option<serde_json::Value>,
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
            meta: result.meta.map(|meta| serde_json::to_value(meta).unwrap()),
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

pub async fn spawn_broker_with_stub_daemon() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code)]
pub async fn spawn_broker_with_live_and_dead_targets() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let dead_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{live_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"

[targets.builder-b]
base_url = "https://{dead_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-b"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code)]
pub async fn spawn_broker_with_retryable_exec_write_error() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_retryable_exec_write_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code)]
pub async fn spawn_broker_with_unknown_session_exec_write_error() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_unknown_session_exec_write_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code)]
pub async fn spawn_broker_with_late_target() -> DelayedTargetFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let delayed_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{live_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"

[targets.builder-b]
base_url = "https://{delayed_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-b"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    DelayedTargetFixture {
        broker: BrokerFixture {
            _tempdir: tempdir,
            client,
            stub_state,
        },
        certs,
        addr: delayed_addr,
    }
}

#[derive(Clone)]
struct StubDaemonState {
    target: String,
    daemon_instance_id: String,
    exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    exec_start_calls: Arc<Mutex<usize>>,
    last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    image_read_response: Arc<Mutex<StubImageReadResponse>>,
}

fn stub_daemon_state(target: &str, exec_write_behavior: ExecWriteBehavior) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: "daemon-instance-1".to_string(),
        exec_write_behavior: Arc::new(Mutex::new(exec_write_behavior)),
        exec_start_warnings: Arc::new(Mutex::new(Vec::new())),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
    }
}

async fn spawn_stub_daemon(certs: &TestCerts) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::Success).await
}

#[allow(dead_code)]
async fn spawn_retryable_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::TemporaryFailureOnce).await
}

#[allow(dead_code)]
async fn spawn_unknown_session_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::UnknownSession).await
}

async fn spawn_daemon(
    certs: &TestCerts,
    exec_write_behavior: ExecWriteBehavior,
) -> (std::net::SocketAddr, StubDaemonState) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let state = stub_daemon_state("builder-a", exec_write_behavior);
    spawn_named_daemon_on_addr(certs, addr, state.clone()).await;
    (addr, state)
}

async fn spawn_named_daemon_on_addr(
    certs: &TestCerts,
    addr: std::net::SocketAddr,
    state: StubDaemonState,
) {
    let app = Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(exec_start))
        .route("/v1/exec/write", post(exec_write))
        .route("/v1/patch/apply", post(patch_apply))
        .route("/v1/image/read", post(image_read))
        .with_state(state.clone());

    let daemon_state = remote_exec_daemon::AppState {
        config: Arc::new(remote_exec_daemon::config::DaemonConfig {
            target: state.target.clone(),
            listen: addr,
            default_workdir: PathBuf::from("."),
            allow_login_shell: true,
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: certs.daemon_cert.clone(),
                key_pem: certs.daemon_key.clone(),
                ca_pem: certs.ca_cert.clone(),
            },
        }),
        daemon_instance_id: "daemon-instance-1".to_string(),
        sessions: remote_exec_daemon::exec::store::SessionStore::default(),
    };

    tokio::spawn(async move {
        remote_exec_daemon::tls::serve_tls(app, Arc::new(daemon_state))
            .await
            .unwrap();
    });

    wait_until_ready(certs, addr).await;
}

async fn health() -> Json<HealthCheckResponse> {
    Json(HealthCheckResponse {
        status: "ok".to_string(),
        daemon_version: "0.1.0".to_string(),
        daemon_instance_id: "daemon-instance-1".to_string(),
    })
}

async fn target_info(State(state): State<StubDaemonState>) -> Json<TargetInfoResponse> {
    Json(TargetInfoResponse {
        target: state.target,
        daemon_version: "0.1.0".to_string(),
        daemon_instance_id: state.daemon_instance_id,
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        supports_pty: true,
        supports_image_read: true,
    })
}

async fn exec_start(
    State(state): State<StubDaemonState>,
    Json(_req): Json<ExecStartRequest>,
) -> Json<ExecResponse> {
    *state.exec_start_calls.lock().await += 1;
    let warnings = state.exec_start_warnings.lock().await.clone();

    Json(ExecResponse {
        daemon_session_id: Some("daemon-session-1".to_string()),
        daemon_instance_id: "daemon-instance-1".to_string(),
        running: true,
        chunk_id: Some("chunk-start".to_string()),
        wall_time_seconds: 0.25,
        exit_code: None,
        original_token_count: Some(1),
        output: "ready".to_string(),
        warnings,
    })
}

async fn exec_write(
    State(state): State<StubDaemonState>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    assert_eq!(req.daemon_session_id, "daemon-session-1");
    let mut behavior = state.exec_write_behavior.lock().await;
    match *behavior {
        ExecWriteBehavior::Success => {}
        ExecWriteBehavior::TemporaryFailureOnce => {
            *behavior = ExecWriteBehavior::Success;
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RpcErrorBody {
                    code: "temporary_failure".to_string(),
                    message: "temporary failure".to_string(),
                }),
            ));
        }
        ExecWriteBehavior::UnknownSession => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(RpcErrorBody {
                    code: "unknown_session".to_string(),
                    message: "Unknown daemon session".to_string(),
                }),
            ));
        }
    }
    drop(behavior);

    Ok(Json(ExecResponse {
        daemon_session_id: None,
        daemon_instance_id: state.daemon_instance_id,
        running: false,
        chunk_id: Some("chunk-write".to_string()),
        wall_time_seconds: 0.5,
        exit_code: Some(0),
        original_token_count: Some(2),
        output: "poll output".to_string(),
        warnings: Vec::new(),
    }))
}

async fn patch_apply(
    State(state): State<StubDaemonState>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    *state.last_patch_request.lock().await = Some(req.clone());
    if !req.patch.starts_with("*** Begin Patch\n") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "patch_failed".to_string(),
                message: "invalid patch header".to_string(),
            }),
        ));
    }

    Ok(Json(PatchApplyResponse {
        output: "Success. Updated the following files:\nA hello.txt\n".to_string(),
    }))
}

async fn image_read(
    State(state): State<StubDaemonState>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    match state.image_read_response.lock().await.clone() {
        StubImageReadResponse::Success(mut response) => {
            response.detail = req.detail.filter(|value| value == "original");
            Ok(Json(response))
        }
        StubImageReadResponse::Error { status, body } => Err((status, Json(body))),
    }
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

#[allow(dead_code)]
fn allocate_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

async fn wait_until_ready(certs: &TestCerts, addr: std::net::SocketAddr) {
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

    for _ in 0..40 {
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

    panic!("stub daemon did not become ready");
}
