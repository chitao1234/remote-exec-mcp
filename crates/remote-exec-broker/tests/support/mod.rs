use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, HealthCheckResponse, TargetInfoResponse,
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
}

impl BrokerFixture {
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
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

#[allow(dead_code)]
pub struct ToolResult {
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
        let raw_content = result
            .content
            .iter()
            .map(|content| serde_json::to_value(content).unwrap())
            .collect();

        Self {
            text_output,
            structured_content: result.structured_content.unwrap_or(serde_json::Value::Null),
            raw_content,
        }
    }
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
    let addr = spawn_stub_daemon(&certs).await;
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
    }
}

#[derive(Clone)]
struct StubDaemonState {
    target: String,
    daemon_instance_id: String,
}

async fn spawn_stub_daemon(certs: &TestCerts) -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let state = StubDaemonState {
        target: "builder-a".to_string(),
        daemon_instance_id: "daemon-instance-1".to_string(),
    };
    let app = Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(exec_start))
        .route("/v1/exec/write", post(exec_write))
        .with_state(state);

    let daemon_state = remote_exec_daemon::AppState {
        config: Arc::new(remote_exec_daemon::config::DaemonConfig {
            target: "builder-a".to_string(),
            listen: addr,
            default_workdir: PathBuf::from("."),
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: certs.daemon_cert.clone(),
                key_pem: certs.daemon_key.clone(),
                ca_pem: certs.ca_cert.clone(),
            },
        }),
        daemon_instance_id: "daemon-instance-1".to_string(),
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    tokio::spawn(async move {
        remote_exec_daemon::tls::serve_tls(app, Arc::new(daemon_state))
            .await
            .unwrap();
    });

    wait_until_ready(certs, addr).await;
    addr
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
        hostname: "stub-host".to_string(),
        platform: "linux".to_string(),
        arch: "x86_64".to_string(),
        supports_pty: true,
        supports_image_read: true,
    })
}

async fn exec_start(Json(_req): Json<ExecStartRequest>) -> Json<ExecResponse> {
    Json(ExecResponse {
        daemon_session_id: Some("daemon-session-1".to_string()),
        running: true,
        chunk_id: Some("chunk-start".to_string()),
        wall_time_seconds: 0.25,
        exit_code: None,
        original_token_count: Some(1),
        output: "ready".to_string(),
    })
}

async fn exec_write(Json(req): Json<ExecWriteRequest>) -> Json<ExecResponse> {
    assert_eq!(req.daemon_session_id, "daemon-session-1");
    Json(ExecResponse {
        daemon_session_id: None,
        running: false,
        chunk_id: Some("chunk-write".to_string()),
        wall_time_seconds: 0.5,
        exit_code: Some(0),
        original_token_count: Some(2),
        output: "poll output".to_string(),
    })
}

struct TestCerts {
    ca_cert: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
    daemon_cert: PathBuf,
    daemon_key: PathBuf,
}

fn write_test_certs(dir: &Path) -> TestCerts {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca_cert = rcgen::CertificateParams::new(vec![])
        .unwrap()
        .self_signed(&ca_key)
        .unwrap();

    let mut daemon_params =
        rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    daemon_params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap()));
    let daemon_key = rcgen::KeyPair::generate().unwrap();
    let daemon_cert = daemon_params
        .signed_by(&daemon_key, &ca_cert, &ca_key)
        .unwrap();

    let client_key = rcgen::KeyPair::generate().unwrap();
    let client_cert = rcgen::CertificateParams::new(vec!["broker".to_string()])
        .unwrap()
        .signed_by(&client_key, &ca_cert, &ca_key)
        .unwrap();

    let ca_cert_path = dir.join("ca.pem");
    let daemon_cert_path = dir.join("daemon.pem");
    let daemon_key_path = dir.join("daemon.key");
    let client_cert_path = dir.join("client.pem");
    let client_key_path = dir.join("client.key");

    std::fs::write(&ca_cert_path, ca_cert.pem()).unwrap();
    std::fs::write(&daemon_cert_path, daemon_cert.pem()).unwrap();
    std::fs::write(&daemon_key_path, daemon_key.serialize_pem()).unwrap();
    std::fs::write(&client_cert_path, client_cert.pem()).unwrap();
    std::fs::write(&client_key_path, client_key.serialize_pem()).unwrap();

    TestCerts {
        ca_cert: ca_cert_path,
        client_cert: client_cert_path,
        client_key: client_key_path,
        daemon_cert: daemon_cert_path,
        daemon_key: daemon_key_path,
    }
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    panic!("stub daemon did not become ready");
}
