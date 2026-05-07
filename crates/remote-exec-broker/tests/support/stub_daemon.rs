use std::collections::HashSet;
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONNECTION, UPGRADE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use remote_exec_host::{
    HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig, build_runtime_state,
};
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN,
    read_frame, read_preface, write_frame, write_preface,
};
use remote_exec_proto::rpc::{
    ExecWarning, HealthCheckResponse, ImageReadResponse, PatchApplyRequest, PatchApplyResponse,
    RpcErrorBody, TargetInfoResponse,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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
pub(crate) use stub_daemon_exec::{set_exec_start_behavior, set_exec_write_behavior};
pub(crate) use stub_daemon_image::StubImageReadResponse;
pub(crate) use stub_daemon_image::set_image_read_response;
pub(crate) use stub_daemon_transfer::{
    StubTransferExportCapture, StubTransferImportCapture, StubTransferPathInfoResponse,
};
pub(crate) use stub_daemon_transfer::{
    set_transfer_export_directory_response, set_transfer_export_file_response,
    set_transfer_path_info_error_response, set_transfer_path_info_response,
};

#[derive(Clone)]
enum ResumeBehavior {
    PassThrough,
    DropTransport,
    SendError { code: String, message: String },
}

#[derive(Clone, Copy)]
enum TcpConnectOkBehavior {
    PassThrough,
    DropAll,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpConnectorStats {
    pub active: usize,
    pub max_observed: usize,
    pub opened: usize,
}

#[derive(Clone)]
struct StubPortTunnelControl {
    enabled: bool,
    resume_behavior: ResumeBehavior,
    tcp_connect_ok_behavior: TcpConnectOkBehavior,
    close_transports_on_second_session_open: bool,
    session_open_count: usize,
    delay_session_ready_after_first: Option<Duration>,
    session_ready_count: usize,
    active_transports: Vec<CancellationToken>,
    active_udp_connector_streams: HashSet<u32>,
    max_observed_udp_connector_streams: usize,
    opened_udp_connector_streams: usize,
}

impl Default for StubPortTunnelControl {
    fn default() -> Self {
        Self {
            enabled: false,
            resume_behavior: ResumeBehavior::PassThrough,
            tcp_connect_ok_behavior: TcpConnectOkBehavior::PassThrough,
            close_transports_on_second_session_open: false,
            session_open_count: 0,
            delay_session_ready_after_first: None,
            session_ready_count: 0,
            active_transports: Vec::new(),
            active_udp_connector_streams: HashSet::new(),
            max_observed_udp_connector_streams: 0,
            opened_udp_connector_streams: 0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct StubDaemonState {
    pub(super) target: String,
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    target_hostname: String,
    target_platform: String,
    target_arch: String,
    target_supports_pty: bool,
    pub(super) target_supports_transfer_compression: bool,
    target_supports_port_forward: bool,
    target_port_forward_protocol_version: u32,
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
    port_tunnel_state: Arc<remote_exec_host::HostRuntimeState>,
    port_tunnel_control: Arc<Mutex<StubPortTunnelControl>>,
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
        target_supports_port_forward: false,
        target_port_forward_protocol_version: 0,
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
        port_tunnel_state: build_stub_port_tunnel_state(target),
        port_tunnel_control: Arc::new(Mutex::new(StubPortTunnelControl::default())),
    }
}

pub(super) fn set_transfer_compression_support(state: &mut StubDaemonState, enabled: bool) {
    state.target_supports_transfer_compression = enabled;
}

pub(super) fn set_port_forward_support(state: &mut StubDaemonState, enabled: bool, version: u32) {
    state.target_supports_port_forward = enabled;
    state.target_port_forward_protocol_version = version;
}

pub(super) fn set_required_bearer_token(state: &mut StubDaemonState, token: &str) {
    state.required_bearer_token = Some(token.to_string());
}

pub(crate) async fn enable_reconnectable_port_tunnel(state: &StubDaemonState) {
    state.port_tunnel_control.lock().await.enabled = true;
}

pub(crate) async fn force_close_port_tunnel_transport(state: &StubDaemonState) {
    let active_transports = {
        let mut control = state.port_tunnel_control.lock().await;
        std::mem::take(&mut control.active_transports)
    };
    for transport in active_transports {
        transport.cancel();
    }
}

pub(crate) async fn block_session_resume(state: &StubDaemonState) {
    state.port_tunnel_control.lock().await.resume_behavior = ResumeBehavior::DropTransport;
}

pub(crate) async fn set_port_tunnel_resume_error(
    state: &StubDaemonState,
    code: &str,
    message: &str,
) {
    state.port_tunnel_control.lock().await.resume_behavior = ResumeBehavior::SendError {
        code: code.to_string(),
        message: message.to_string(),
    };
}

pub(crate) async fn drop_tcp_connect_ok_frames(state: &StubDaemonState) {
    state
        .port_tunnel_control
        .lock()
        .await
        .tcp_connect_ok_behavior = TcpConnectOkBehavior::DropAll;
}

pub(crate) async fn fail_first_forward_runtime_before_multi_open_finishes(state: &StubDaemonState) {
    let mut control = state.port_tunnel_control.lock().await;
    control.close_transports_on_second_session_open = true;
    control.delay_session_ready_after_first = Some(Duration::from_millis(500));
    control.resume_behavior = ResumeBehavior::SendError {
        code: "forced_resume_failure".to_string(),
        message: "forced resume failure".to_string(),
    };
}

pub(crate) async fn udp_connector_stats(state: &StubDaemonState) -> UdpConnectorStats {
    let control = state.port_tunnel_control.lock().await;
    UdpConnectorStats {
        active: control.active_udp_connector_streams.len(),
        max_observed: control.max_observed_udp_connector_streams,
        opened: control.opened_udp_connector_streams,
    }
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
        .route("/v1/port/tunnel", post(port_tunnel))
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
        supports_port_forward: state.target_supports_port_forward,
        port_forward_protocol_version: state.target_port_forward_protocol_version,
    })
}

async fn port_tunnel(
    State(state): State<StubDaemonState>,
    headers: HeaderMap,
    request: Request,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    {
        let control = state.port_tunnel_control.lock().await;
        if !control.enabled {
            return Err(unsupported_port_tunnel_request());
        }
    }

    validate_port_tunnel_upgrade_headers(&headers)?;
    let on_upgrade = upgrade::on(request);
    let handler_state = state.clone();

    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                if let Err(err) =
                    handle_port_tunnel_upgrade(handler_state, TokioIo::new(upgraded)).await
                {
                    tracing::warn!(error = %err, "stub port tunnel upgrade failed");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "stub port tunnel upgrade failed");
            }
        }
    });

    Ok((
        StatusCode::SWITCHING_PROTOCOLS,
        [(CONNECTION, "Upgrade"), (UPGRADE, UPGRADE_TOKEN)],
        Body::empty(),
    )
        .into_response())
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

fn build_stub_port_tunnel_state(target: &str) -> Arc<remote_exec_host::HostRuntimeState> {
    let workdir = std::env::temp_dir().join(format!(
        "remote-exec-broker-stub-port-tunnel-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workdir).unwrap();
    Arc::new(
        build_runtime_state(HostRuntimeConfig {
            target: target.to_string(),
            default_workdir: workdir,
            windows_posix_root: None,
            sandbox: None,
            enable_transfer_compression: true,
            allow_login_shell: true,
            pty: PtyMode::None,
            default_shell: None,
            yield_time: YieldTimeConfig::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: ProcessEnvironment::capture_current(),
        })
        .unwrap(),
    )
}

async fn handle_port_tunnel_upgrade<S>(state: StubDaemonState, mut stream: S) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    read_preface(&mut stream).await?;
    let first_frame = read_frame(&mut stream).await?;

    if first_frame.frame_type == FrameType::SessionResume {
        match state
            .port_tunnel_control
            .lock()
            .await
            .resume_behavior
            .clone()
        {
            ResumeBehavior::PassThrough => {}
            ResumeBehavior::DropTransport => return Ok(()),
            ResumeBehavior::SendError { code, message } => {
                write_frame(
                    &mut stream,
                    &Frame {
                        frame_type: FrameType::Error,
                        flags: 0,
                        stream_id: 0,
                        meta: serde_json::to_vec(&serde_json::json!({
                            "code": code,
                            "message": message,
                            "fatal": false,
                        }))?,
                        data: Vec::new(),
                    },
                )
                .await?;
                return Ok(());
            }
        }
    }

    let (mut broker_side, daemon_side) = tokio::io::duplex(256 * 1024);
    let tunnel_state = state.port_tunnel_state.clone();
    tokio::spawn(async move {
        let _ = remote_exec_host::port_forward::serve_tunnel(tunnel_state, daemon_side).await;
    });
    write_preface(&mut broker_side).await?;
    observe_broker_to_daemon_frame(&state, &first_frame).await;
    write_frame(&mut broker_side, &first_frame).await?;

    let cancel = CancellationToken::new();
    state
        .port_tunnel_control
        .lock()
        .await
        .active_transports
        .push(cancel.clone());

    relay_port_tunnel_frames(state, stream, broker_side, cancel).await
}

async fn relay_port_tunnel_frames<S1, S2>(
    state: StubDaemonState,
    mut external: S1,
    mut internal: S2,
    cancel: CancellationToken,
) -> anyhow::Result<()>
where
    S1: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    S2: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            frame = read_frame(&mut external) => {
                let Some(frame) = frame_from_result(frame)? else {
                    return Ok(());
                };
                observe_broker_to_daemon_frame(&state, &frame).await;
                write_frame(&mut internal, &frame).await?;
            }
            frame = read_frame(&mut internal) => {
                let Some(frame) = frame_from_result(frame)? else {
                    return Ok(());
                };
                if should_forward_daemon_to_broker_frame(&state, &frame).await {
                    write_frame(&mut external, &frame).await?;
                }
            }
        }
    }
}

fn frame_from_result(result: std::io::Result<Frame>) -> anyhow::Result<Option<Frame>> {
    match result {
        Ok(frame) => Ok(Some(frame)),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn observe_broker_to_daemon_frame(state: &StubDaemonState, frame: &Frame) {
    let transports_to_cancel = {
        let mut control = state.port_tunnel_control.lock().await;
        match frame.frame_type {
            FrameType::SessionOpen => {
                control.session_open_count += 1;
                if control.close_transports_on_second_session_open
                    && control.session_open_count == 2
                {
                    control.close_transports_on_second_session_open = false;
                    std::mem::take(&mut control.active_transports)
                } else {
                    Vec::new()
                }
            }
            FrameType::UdpBind if frame.stream_id >= 3 => {
                control.active_udp_connector_streams.insert(frame.stream_id);
                control.opened_udp_connector_streams += 1;
                control.max_observed_udp_connector_streams = control
                    .max_observed_udp_connector_streams
                    .max(control.active_udp_connector_streams.len());
                Vec::new()
            }
            FrameType::Close => {
                control
                    .active_udp_connector_streams
                    .remove(&frame.stream_id);
                Vec::new()
            }
            _ => Vec::new(),
        }
    };
    for transport in transports_to_cancel {
        transport.cancel();
    }
}

async fn should_forward_daemon_to_broker_frame(state: &StubDaemonState, frame: &Frame) -> bool {
    let (delay, should_forward) = {
        let mut control = state.port_tunnel_control.lock().await;
        if frame.frame_type == FrameType::Close {
            control
                .active_udp_connector_streams
                .remove(&frame.stream_id);
        }
        let delay = if frame.frame_type == FrameType::SessionReady {
            let delay = if control.session_ready_count > 0 {
                control.delay_session_ready_after_first
            } else {
                None
            };
            control.session_ready_count += 1;
            delay
        } else {
            None
        };
        let should_forward = !(frame.frame_type == FrameType::TcpConnectOk
            && matches!(
                control.tcp_connect_ok_behavior,
                TcpConnectOkBehavior::DropAll
            ));
        (delay, should_forward)
    };
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    should_forward
}

fn validate_port_tunnel_upgrade_headers(
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<RpcErrorBody>)> {
    if !header_contains_token(headers, CONNECTION.as_str(), "upgrade") {
        return Err(bad_port_tunnel_request(
            "missing `Connection: Upgrade` header",
        ));
    }
    if !header_eq(headers, UPGRADE.as_str(), UPGRADE_TOKEN) {
        return Err(bad_port_tunnel_request(format!(
            "missing `Upgrade: {UPGRADE_TOKEN}` header"
        )));
    }
    if !header_eq(
        headers,
        TUNNEL_PROTOCOL_VERSION_HEADER,
        TUNNEL_PROTOCOL_VERSION,
    ) {
        return Err(bad_port_tunnel_request(format!(
            "missing `{TUNNEL_PROTOCOL_VERSION_HEADER}: {TUNNEL_PROTOCOL_VERSION}` header"
        )));
    }
    Ok(())
}

fn header_contains_token(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(expected))
}

fn header_eq(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn unsupported_port_tunnel_request() -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code: "unsupported_operation".to_string(),
            message: "stub port tunnel support is disabled".to_string(),
        }),
    )
}

fn bad_port_tunnel_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code: "bad_request".to_string(),
            message: message.into(),
        }),
    )
}
