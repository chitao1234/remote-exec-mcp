#![allow(dead_code)]

use std::path::PathBuf;
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
use tokio::sync::Mutex;

use super::certs::{TestCerts, allocate_addr};

#[derive(Debug, Clone)]
pub enum StubImageReadResponse {
    Success(ImageReadResponse),
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ExecWriteBehavior {
    Success,
    TemporaryFailureOnce,
    UnknownSession,
}

#[derive(Clone)]
pub(super) struct StubDaemonState {
    pub(super) target: String,
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    target_hostname: String,
    target_platform: String,
    target_arch: String,
    target_supports_pty: bool,
    exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    pub(super) exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    pub(super) exec_start_calls: Arc<Mutex<usize>>,
    pub(super) last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    pub(super) image_read_response: Arc<Mutex<StubImageReadResponse>>,
}

pub(super) fn stub_daemon_state(
    target: &str,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: Arc::new(Mutex::new("daemon-instance-1".to_string())),
        target_hostname: format!("{target}-host"),
        target_platform: platform.to_string(),
        target_arch: "x86_64".to_string(),
        target_supports_pty: supports_pty,
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

pub(super) async fn spawn_stub_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::Success).await
}

pub(super) async fn spawn_retryable_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::TemporaryFailureOnce).await
}

pub(super) async fn spawn_unknown_session_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::UnknownSession).await
}

async fn spawn_daemon(
    certs: &TestCerts,
    exec_write_behavior: ExecWriteBehavior,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon_with_platform(certs, exec_write_behavior, "linux", true).await
}

pub(super) async fn spawn_daemon_with_platform(
    certs: &TestCerts,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> (std::net::SocketAddr, StubDaemonState) {
    let addr = allocate_addr();
    let state = stub_daemon_state("builder-a", exec_write_behavior, platform, supports_pty);
    spawn_named_daemon_on_addr(certs, addr, state.clone()).await;
    (addr, state)
}

pub(super) async fn spawn_named_daemon_on_addr(
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
            windows_pty_backend_override: None,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
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
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    Json(TargetInfoResponse {
        target: state.target,
        daemon_version: "0.1.0".to_string(),
        daemon_instance_id,
        hostname: state.target_hostname,
        platform: state.target_platform,
        arch: state.target_arch,
        supports_pty: state.target_supports_pty,
        supports_image_read: true,
    })
}

async fn exec_start(
    State(state): State<StubDaemonState>,
    Json(_req): Json<ExecStartRequest>,
) -> Json<ExecResponse> {
    *state.exec_start_calls.lock().await += 1;
    let warnings = state.exec_start_warnings.lock().await.clone();
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    Json(ExecResponse {
        daemon_session_id: Some("daemon-session-1".to_string()),
        daemon_instance_id,
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
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    Ok(Json(ExecResponse {
        daemon_session_id: None,
        daemon_instance_id,
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
