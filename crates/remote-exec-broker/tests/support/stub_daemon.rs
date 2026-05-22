use std::future::Future;
use std::num::NonZeroU32;
#[cfg(feature = "broker-tls")]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::FutureExt;
use remote_exec_proto::rpc::{
    DaemonIdentity, ExecWarning, ExecWriteRequest, FileToolProtocolVersion, HealthCheckResponse,
    HealthStatus, ImageReadResponse, PatchApplyRequest, PatchApplyResponse,
    PortForwardProtocolVersion, RpcErrorBody, RpcErrorCode, TargetCapabilities, TargetInfoResponse,
    TransferStreamProtocolVersion,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[cfg(feature = "broker-tls")]
use super::certs::TestCerts;
use super::test_helpers::DEFAULT_TEST_TARGET;

#[path = "stub_daemon_exec.rs"]
mod stub_daemon_exec;
#[path = "stub_daemon_file.rs"]
mod stub_daemon_file;
#[path = "stub_daemon_image.rs"]
mod stub_daemon_image;
#[path = "stub_daemon_port_forward.rs"]
mod stub_daemon_port_forward;
#[path = "stub_daemon_transfer.rs"]
mod stub_daemon_transfer;

pub(crate) use stub_daemon_exec::{ExecStartBehavior, ExecWriteBehavior};
pub(crate) use stub_daemon_exec::{set_exec_start_behavior, set_exec_write_behavior};
pub(crate) use stub_daemon_file::{
    StubFileEditResponse, StubFileReadResponse, StubFileWriteResponse, set_file_edit_response,
    set_file_read_response, set_file_write_response,
};
pub(crate) use stub_daemon_image::StubImageReadResponse;
pub(crate) use stub_daemon_image::set_image_read_response;
use stub_daemon_port_forward::StubPortForwardState;
#[allow(
    unused_imports,
    reason = "Different broker integration tests use different port-forward stub helpers"
)]
pub(crate) use stub_daemon_port_forward::{
    UdpConnectorStats, assert_port_tunnel_relay_preserves_partial_frame_reads,
    block_connect_tunnel_open_after_first, block_session_ready_after_first, block_session_resume,
    closed_port_tunnel_transport_count, delay_session_ready_after_first,
    drop_next_connect_tunnel_opens, drop_next_port_tunnel_heartbeat_ack,
    drop_tcp_connect_ok_frames, enable_reconnectable_port_tunnel,
    fail_first_forward_runtime_before_multi_open_finishes, fail_next_port_tunnel_upgrades,
    fail_next_udp_connector_bind, force_close_connect_port_tunnel_transport,
    force_close_listen_port_tunnel_transport, force_close_port_tunnel_transport,
    hang_session_resume, override_tunnel_ready_limits, resume_attempt_count,
    set_port_tunnel_resume_error, set_session_resume_timeout, tunnel_open_count,
    udp_connector_stats, wait_for_closed_port_tunnel_transports,
    wait_for_connect_port_tunnel_transports, wait_for_listen_port_tunnel_transports,
    wait_for_port_tunnel_transports, wait_for_resume_attempts,
};
pub(crate) use stub_daemon_transfer::{
    StubTransferExportCapture, StubTransferImportCapture, StubTransferPathInfoResponse,
};
pub(crate) use stub_daemon_transfer::{
    set_transfer_export_directory_response, set_transfer_export_file_response,
    set_transfer_path_info_error_response, set_transfer_path_info_response,
};

const STUB_READY_TIMEOUT: Duration = Duration::from_secs(5);
const STUB_READY_POLL: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub(crate) struct StubDaemonState {
    pub(super) target: String,
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    pub(super) daemon_session_id: Arc<Mutex<String>>,
    target_hostname: String,
    target_platform: String,
    target_arch: String,
    target_supports_pty: bool,
    pub(super) target_supports_transfer_compression: bool,
    target_supports_port_forward: bool,
    target_port_forward_protocol_version: Option<PortForwardProtocolVersion>,
    required_bearer_token: Option<String>,
    pub(super) exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    pub(super) exec_start_behavior: Arc<Mutex<ExecStartBehavior>>,
    pub(super) exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    pub(super) exec_start_calls: Arc<Mutex<usize>>,
    pub(super) last_exec_write_request: Arc<Mutex<Option<ExecWriteRequest>>>,
    pub(super) last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    pub(super) last_file_read_request: Arc<Mutex<Option<remote_exec_proto::rpc::FileReadRequest>>>,
    pub(super) last_file_write_request:
        Arc<Mutex<Option<remote_exec_proto::rpc::FileWriteRequest>>>,
    pub(super) last_file_edit_request: Arc<Mutex<Option<remote_exec_proto::rpc::FileEditRequest>>>,
    pub(super) last_transfer_import: Arc<Mutex<Option<StubTransferImportCapture>>>,
    pub(super) last_transfer_export: Arc<Mutex<Option<StubTransferExportCapture>>>,
    pub(super) image_read_response: Arc<Mutex<StubImageReadResponse>>,
    pub(super) file_read_response: Arc<Mutex<StubFileReadResponse>>,
    pub(super) file_write_response: Arc<Mutex<StubFileWriteResponse>>,
    pub(super) file_edit_response: Arc<Mutex<StubFileEditResponse>>,
    transfer_export_response: Arc<Mutex<stub_daemon_transfer::StubTransferExportResponse>>,
    transfer_path_info_response: Arc<Mutex<StubTransferPathInfoResponse>>,
    port_forward: StubPortForwardState,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

pub(super) fn stub_daemon_state(
    target: &str,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: Arc::new(Mutex::new(remote_exec_host::ids::new_instance_id())),
        daemon_session_id: Arc::new(Mutex::new(remote_exec_host::ids::new_exec_session_id())),
        target_hostname: format!("{target}-host"),
        target_platform: platform.to_string(),
        target_arch: "x86_64".to_string(),
        target_supports_pty: supports_pty,
        target_supports_transfer_compression: true,
        target_supports_port_forward: false,
        target_port_forward_protocol_version: None,
        required_bearer_token: None,
        exec_write_behavior: Arc::new(Mutex::new(exec_write_behavior)),
        exec_start_behavior: Arc::new(Mutex::new(ExecStartBehavior::Success)),
        exec_start_warnings: Arc::new(Mutex::new(Vec::new())),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_exec_write_request: Arc::new(Mutex::new(None)),
        last_patch_request: Arc::new(Mutex::new(None)),
        last_file_read_request: Arc::new(Mutex::new(None)),
        last_file_write_request: Arc::new(Mutex::new(None)),
        last_file_edit_request: Arc::new(Mutex::new(None)),
        last_transfer_import: Arc::new(Mutex::new(None)),
        last_transfer_export: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
        file_read_response: Arc::new(Mutex::new(StubFileReadResponse::Success(
            remote_exec_proto::rpc::FileReadResponse {
                output: "1: hello\n\n(EOF reached, file has 1 lines)".to_string(),
                lines_returned: 1,
                total_lines: 1,
                eof: true,
            },
        ))),
        file_write_response: Arc::new(Mutex::new(StubFileWriteResponse::Success(
            remote_exec_proto::rpc::FileWriteResponse {
                created: false,
                line_count: 1,
            },
        ))),
        file_edit_response: Arc::new(Mutex::new(StubFileEditResponse::Success(
            remote_exec_proto::rpc::FileEditResponse {
                replacements: 1,
                line_count: 1,
            },
        ))),
        transfer_export_response: Arc::new(Mutex::new(
            stub_daemon_transfer::default_transfer_export_response(),
        )),
        transfer_path_info_response: Arc::new(Mutex::new(
            stub_daemon_transfer::default_transfer_path_info_response(),
        )),
        port_forward: StubPortForwardState::new(target),
        background_tasks: Arc::new(Mutex::new(Vec::new())),
    }
}

pub(super) fn set_transfer_compression_support(state: &mut StubDaemonState, enabled: bool) {
    state.target_supports_transfer_compression = enabled;
}

pub(super) fn set_port_forward_support(state: &mut StubDaemonState, enabled: bool, version: u32) {
    state.target_supports_port_forward = enabled;
    state.target_port_forward_protocol_version = if enabled {
        NonZeroU32::new(version).map(PortForwardProtocolVersion::new)
    } else {
        None
    };
}

pub(super) fn set_required_bearer_token(state: &mut StubDaemonState, token: &str) {
    state.required_bearer_token = Some(token.to_string());
}

async fn spawn_stub_task(
    state: &StubDaemonState,
    name: &'static str,
    task: impl Future<Output = anyhow::Result<()>> + Send + 'static,
) {
    let handle = tokio::spawn(async move {
        match std::panic::AssertUnwindSafe(task).catch_unwind().await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => panic!("stub daemon background task `{name}` failed: {err:?}"),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    });
    state.background_tasks.lock().await.push(handle);
}

pub(crate) async fn assert_no_stub_task_panics(state: &StubDaemonState) {
    let finished = {
        let mut tasks = state.background_tasks.lock().await;
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
        handle.await.expect("stub daemon background task panicked");
    }
}

#[cfg(feature = "broker-tls")]
pub(super) async fn spawn_stub_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::Success).await
}

pub(super) async fn spawn_plain_http_stub_daemon() -> (std::net::SocketAddr, StubDaemonState) {
    spawn_plain_http_daemon(ExecWriteBehavior::Success).await
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
#[cfg(feature = "broker-tls")]
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
#[cfg(feature = "broker-tls")]
pub(super) async fn spawn_unknown_session_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::UnknownSession).await
}

pub(super) async fn spawn_plain_http_unknown_session_exec_write_daemon()
-> (std::net::SocketAddr, StubDaemonState) {
    spawn_plain_http_daemon(ExecWriteBehavior::UnknownSession).await
}

#[cfg(feature = "broker-tls")]
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

#[cfg(feature = "broker-tls")]
pub(super) async fn spawn_daemon_with_platform(
    certs: &TestCerts,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> (std::net::SocketAddr, StubDaemonState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind TLS stub daemon listener");
    let addr = listener.local_addr().expect("read TLS stub daemon addr");
    let state = stub_daemon_state(
        DEFAULT_TEST_TARGET,
        exec_write_behavior,
        platform,
        supports_pty,
    );
    spawn_named_daemon_on_listener(certs, listener, state.clone()).await;
    (addr, state)
}

pub(super) async fn spawn_plain_http_daemon_with_platform(
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> (std::net::SocketAddr, StubDaemonState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = stub_daemon_state(
        DEFAULT_TEST_TARGET,
        exec_write_behavior,
        platform,
        supports_pty,
    );
    spawn_named_plain_http_daemon_on_listener(listener, state.clone()).await;
    (addr, state)
}

#[cfg(feature = "broker-tls")]
pub(super) async fn spawn_named_daemon_on_listener(
    certs: &TestCerts,
    listener: tokio::net::TcpListener,
    state: StubDaemonState,
) {
    let addr = listener.local_addr().expect("read TLS stub daemon addr");
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
        transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
        max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
        allow_login_shell: true,
        pty: remote_exec_daemon::config::PtyMode::Auto,
        default_shell: None,
        yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
        port_forward_limits: remote_exec_daemon::config::HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: None,
        }),
        request_timeout_ms: 300_000,
    };

    let task_state = state.clone();
    spawn_stub_task(&state, "tls-server", async move {
        remote_exec_daemon::test_support::serve_tls_on_listener(
            app,
            Arc::new(daemon_config),
            listener,
            std::future::pending::<()>(),
        )
        .await
    })
    .await;
    wait_until_ready(certs, addr).await;
    assert_no_stub_task_panics(&task_state).await;
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
    let app = stub_router(state.clone());

    let task_state = state.clone();
    spawn_stub_task(&state, "plain-http-server", async move {
        axum::serve(listener, app).await.map_err(Into::into)
    })
    .await;
    wait_until_ready_http(addr).await;
    assert_no_stub_task_panics(&task_state).await;
}

pub(crate) async fn spawn_plain_http_stub_on_listener(
    listener: tokio::net::TcpListener,
    state: StubDaemonState,
) {
    spawn_named_plain_http_daemon_on_listener(listener, state).await;
}

pub(super) fn stub_router(state: StubDaemonState) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route(
            "/v1/port/tunnel",
            post(stub_daemon_port_forward::port_tunnel),
        )
        .route("/v1/exec/start", post(stub_daemon_exec::exec_start))
        .route("/v1/exec/write", post(stub_daemon_exec::exec_write))
        .route("/v1/patch/apply", post(patch_apply))
        .route("/v1/file/read", post(stub_daemon_file::file_read))
        .route("/v1/file/write", post(stub_daemon_file::file_write))
        .route("/v1/file/edit", post(stub_daemon_file::file_edit))
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
        Json(RpcErrorBody::new(
            RpcErrorCode::Unauthorized,
            "missing or invalid bearer token",
        )),
    )
        .into_response()
}

async fn health(State(state): State<StubDaemonState>) -> Json<HealthCheckResponse> {
    Json(HealthCheckResponse {
        status: HealthStatus::Ok,
        daemon_version: "0.1.0".to_string(),
        daemon_instance_id: state.daemon_instance_id.lock().await.clone(),
    })
}

async fn target_info(State(state): State<StubDaemonState>) -> Json<TargetInfoResponse> {
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    Json(TargetInfoResponse {
        target: state.target,
        daemon_instance_id,
        identity: DaemonIdentity {
            daemon_version: "0.1.0".to_string(),
            hostname: state.target_hostname,
            platform: state.target_platform,
            arch: state.target_arch,
        },
        capabilities: TargetCapabilities {
            supports_pty: state.target_supports_pty,
            supports_port_forward: state.target_supports_port_forward,
            port_forward_protocol_version: state.target_port_forward_protocol_version,
            transfer_stream_protocol_version: Some(TransferStreamProtocolVersion::v2()),
            file_tool_protocol_version: Some(FileToolProtocolVersion::v1()),
        },
        supports_image_read: true,
        supports_transfer_compression: state.target_supports_transfer_compression,
    })
}

async fn patch_apply(
    State(state): State<StubDaemonState>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    *state.last_patch_request.lock().await = Some(req.clone());
    let lines = req.patch.lines().collect::<Vec<_>>();
    if lines.first().copied().map(trim_horizontal) != Some("*** Begin Patch") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody::new(
                RpcErrorCode::PatchFailed,
                "invalid patch header",
            )),
        ));
    }
    if lines.last().copied().map(trim_horizontal) != Some("*** End Patch") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody::new(
                RpcErrorCode::PatchFailed,
                "invalid patch footer",
            )),
        ));
    }

    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();
    Ok(Json(PatchApplyResponse {
        output: "Success. Updated the following files:\nA hello.txt\n".to_string(),
        daemon_instance_id: Some(daemon_instance_id),
        updated_paths: vec!["A hello.txt".to_string()],
    }))
}

fn trim_horizontal(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

#[cfg(feature = "broker-tls")]
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

    tokio::time::timeout(STUB_READY_TIMEOUT, async {
        loop {
            if client
                .post(format!("https://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(STUB_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "TLS stub daemon at https://{addr} did not become ready within {STUB_READY_TIMEOUT:?}"
        )
    });
}

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(STUB_READY_TIMEOUT, async {
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
            tokio::time::sleep(STUB_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "plain HTTP stub daemon at http://{addr} did not become ready within {STUB_READY_TIMEOUT:?}"
        )
    });
}
