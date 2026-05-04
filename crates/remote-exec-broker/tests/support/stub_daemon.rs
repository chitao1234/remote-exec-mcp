#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{
    ExecWarning, HealthCheckResponse, ImageReadResponse, PatchApplyRequest, PatchApplyResponse,
    RpcErrorBody, TargetInfoResponse,
};
use tokio::sync::Mutex;

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use super::certs::TestCerts;
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use super::certs::allocate_addr;

#[path = "stub_daemon_exec.rs"]
mod stub_daemon_exec;
#[path = "stub_daemon_image.rs"]
mod stub_daemon_image;
#[path = "stub_daemon_transfer.rs"]
mod stub_daemon_transfer;

pub(crate) use stub_daemon_exec::{ExecStartBehavior, ExecWriteBehavior};
pub(crate) use stub_daemon_image::StubImageReadResponse;
pub(crate) use stub_daemon_transfer::{
    StubTransferExportCapture, StubTransferImportCapture, StubTransferPathInfoResponse,
};
pub(crate) use stub_daemon_exec::{set_exec_start_behavior, set_exec_write_behavior};
pub(crate) use stub_daemon_image::set_image_read_response;
pub(crate) use stub_daemon_transfer::{
    set_transfer_export_directory_response, set_transfer_export_file_response,
    set_transfer_path_info_error_response, set_transfer_path_info_response,
};

#[derive(Clone)]
pub(crate) struct StubDaemonState {
    pub(super) target: String,
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    target_hostname: String,
    target_platform: String,
    target_arch: String,
    target_supports_pty: bool,
    pub(super) target_supports_transfer_compression: bool,
    required_bearer_token: Option<String>,
    pub(super) exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    pub(super) exec_start_behavior: Arc<Mutex<ExecStartBehavior>>,
    pub(super) exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    pub(super) exec_start_calls: Arc<Mutex<usize>>,
    pub(super) last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    pub(super) last_transfer_import: Arc<Mutex<Option<StubTransferImportCapture>>>,
    pub(super) last_transfer_export: Arc<Mutex<Option<StubTransferExportCapture>>>,
    pub(super) image_read_response: Arc<Mutex<StubImageReadResponse>>,
    transfer_export_response: Arc<Mutex<stub_daemon_transfer::StubTransferExportResponse>>,
    transfer_path_info_response: Arc<Mutex<StubTransferPathInfoResponse>>,
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
        target_supports_transfer_compression: true,
        required_bearer_token: None,
        exec_write_behavior: Arc::new(Mutex::new(exec_write_behavior)),
        exec_start_behavior: Arc::new(Mutex::new(ExecStartBehavior::Success)),
        exec_start_warnings: Arc::new(Mutex::new(Vec::new())),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
        last_transfer_import: Arc::new(Mutex::new(None)),
        last_transfer_export: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
        transfer_export_response: Arc::new(Mutex::new(
            stub_daemon_transfer::default_transfer_export_response(),
        )),
        transfer_path_info_response: Arc::new(Mutex::new(
            stub_daemon_transfer::default_transfer_path_info_response(),
        )),
    }
}

pub(super) fn set_transfer_compression_support(state: &mut StubDaemonState, enabled: bool) {
    state.target_supports_transfer_compression = enabled;
}

pub(super) fn set_required_bearer_token(state: &mut StubDaemonState, token: &str) {
    state.required_bearer_token = Some(token.to_string());
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub(super) async fn spawn_stub_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::Success).await
}

pub(super) async fn spawn_plain_http_stub_daemon() -> (std::net::SocketAddr, StubDaemonState) {
    spawn_plain_http_daemon(ExecWriteBehavior::Success).await
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub(super) async fn spawn_retryable_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::TemporaryFailureOnce).await
}

pub(super) async fn spawn_plain_http_retryable_exec_write_daemon()
-> (std::net::SocketAddr, StubDaemonState) {
    spawn_plain_http_daemon(ExecWriteBehavior::TemporaryFailureOnce).await
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub(super) async fn spawn_unknown_session_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::UnknownSession).await
}

pub(super) async fn spawn_plain_http_unknown_session_exec_write_daemon()
-> (std::net::SocketAddr, StubDaemonState) {
    spawn_plain_http_daemon(ExecWriteBehavior::UnknownSession).await
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
async fn spawn_daemon(
    certs: &TestCerts,
    exec_write_behavior: ExecWriteBehavior,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon_with_platform(certs, exec_write_behavior, "linux", true).await
}

async fn spawn_plain_http_daemon(
    exec_write_behavior: ExecWriteBehavior,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_plain_http_daemon_with_platform(exec_write_behavior, "linux", true).await
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
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

pub(super) async fn spawn_plain_http_daemon_with_platform(
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> (std::net::SocketAddr, StubDaemonState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = stub_daemon_state("builder-a", exec_write_behavior, platform, supports_pty);
    spawn_named_plain_http_daemon_on_listener(listener, state.clone()).await;
    (addr, state)
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub(super) async fn spawn_named_daemon_on_addr(
    certs: &TestCerts,
    addr: std::net::SocketAddr,
    state: StubDaemonState,
) {
    let app = stub_router(state.clone());

    let daemon_config = remote_exec_daemon::config::DaemonConfig {
        target: state.target.clone(),
        listen: addr,
        default_workdir: PathBuf::from("."),
        windows_posix_root: None,
        transport: remote_exec_daemon::config::DaemonTransport::Tls,
        http_auth: None,
        sandbox: None,
        enable_transfer_compression: state.target_supports_transfer_compression,
        allow_login_shell: true,
        pty: remote_exec_daemon::config::PtyMode::Auto,
        default_shell: None,
        yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: None,
        }),
    };

    tokio::spawn(async move {
        remote_exec_daemon::tls::serve_tls(app, Arc::new(daemon_config))
            .await
            .unwrap();
    });

    wait_until_ready(certs, addr).await;
}

pub(super) async fn spawn_named_plain_http_daemon_on_addr(
    addr: std::net::SocketAddr,
    state: StubDaemonState,
) {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    spawn_named_plain_http_daemon_on_listener(listener, state).await;
}

pub(super) async fn spawn_named_plain_http_daemon_on_listener(
    listener: tokio::net::TcpListener,
    state: StubDaemonState,
) {
    let addr = listener.local_addr().unwrap();
    let app = stub_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    wait_until_ready_http(addr).await;
}

pub(super) fn stub_router(state: StubDaemonState) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(stub_daemon_exec::exec_start))
        .route("/v1/exec/write", post(stub_daemon_exec::exec_write))
        .route("/v1/patch/apply", post(patch_apply))
        .route(
            "/v1/transfer/path-info",
            post(stub_daemon_transfer::transfer_path_info),
        )
        .route(
            "/v1/transfer/export",
            post(stub_daemon_transfer::transfer_export),
        )
        .route(
            "/v1/transfer/import",
            post(stub_daemon_transfer::transfer_import),
        )
        .route("/v1/image/read", post(stub_daemon_image::image_read))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ))
        .with_state(state)
}

async fn require_bearer_auth(
    State(state): State<StubDaemonState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected_token) = state.required_bearer_token.as_deref() else {
        return next.run(request).await;
    };

    let actual = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let expected = format!("Bearer {expected_token}");
    if actual == Some(expected.as_str()) {
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, "Bearer")],
        Json(RpcErrorBody {
            code: "unauthorized".to_string(),
            message: "missing or invalid bearer token".to_string(),
        }),
    )
        .into_response()
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
        supports_transfer_compression: state.target_supports_transfer_compression,
        supports_port_forward: false,
    })
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

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
async fn wait_until_ready(certs: &TestCerts, addr: std::net::SocketAddr) {
    let ca = reqwest::Certificate::from_pem(&std::fs::read(&certs.ca_cert).unwrap()).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .tls_certs_only([ca])
        .danger_accept_invalid_hostnames(true)
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

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    remote_exec_broker::install_crypto_provider();
    let client = reqwest::Client::builder().build().unwrap();

    for _ in 0..40 {
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

    panic!("plain http stub daemon did not become ready");
}
