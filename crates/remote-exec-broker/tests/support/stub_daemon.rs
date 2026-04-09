use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWarning, ExecWriteRequest, HealthCheckResponse,
    ImageReadRequest, ImageReadResponse, PatchApplyRequest, PatchApplyResponse, RpcErrorBody,
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER, TargetInfoResponse,
    TransferCompression, TransferExportRequest, TransferImportResponse, TransferSourceType,
};
use tar::{Builder, EntryType, Header};
use tokio::sync::Mutex;

use super::certs::{TestCerts, allocate_addr};

const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

#[derive(Debug, Clone)]
pub enum StubImageReadResponse {
    Success(ImageReadResponse),
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Debug, Clone)]
pub struct StubTransferImportCapture {
    pub destination_path: String,
    pub source_type: String,
    pub compression: String,
    pub overwrite: String,
    pub create_parent: String,
    pub body_len: usize,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
enum StubTransferExportResponse {
    Success {
        source_type: TransferSourceType,
        compression: TransferCompression,
        body: Vec<u8>,
    },
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ExecWriteBehavior {
    Success,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    TemporaryFailureOnce,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    UnknownSession,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    MalformedCompletedMissingExitCode,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ExecStartBehavior {
    Success,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    RunningMissingDaemonSessionId,
}

#[derive(Clone)]
pub(super) struct StubDaemonState {
    pub(super) target: String,
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    target_hostname: String,
    target_platform: String,
    target_arch: String,
    target_supports_pty: bool,
    pub(super) target_supports_transfer_compression: bool,
    pub(super) exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    pub(super) exec_start_behavior: Arc<Mutex<ExecStartBehavior>>,
    pub(super) exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    pub(super) exec_start_calls: Arc<Mutex<usize>>,
    pub(super) last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    pub(super) last_transfer_import: Arc<Mutex<Option<StubTransferImportCapture>>>,
    pub(super) image_read_response: Arc<Mutex<StubImageReadResponse>>,
    transfer_export_response: Arc<Mutex<StubTransferExportResponse>>,
}

fn stub_directory_archive() -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());

    let mut root = Header::new_gnu();
    root.set_entry_type(EntryType::Directory);
    root.set_mode(0o755);
    root.set_size(0);
    root.set_cksum();
    builder
        .append_data(&mut root, ".", std::io::empty())
        .unwrap();

    let mut nested = Header::new_gnu();
    nested.set_entry_type(EntryType::Directory);
    nested.set_mode(0o755);
    nested.set_size(0);
    nested.set_cksum();
    builder
        .append_data(&mut nested, "nested", std::io::empty())
        .unwrap();

    let mut empty = Header::new_gnu();
    empty.set_entry_type(EntryType::Directory);
    empty.set_mode(0o755);
    empty.set_size(0);
    empty.set_cksum();
    builder
        .append_data(&mut empty, "nested/empty", std::io::empty())
        .unwrap();

    let body = b"hello remote\n";
    let mut file = Header::new_gnu();
    file.set_entry_type(EntryType::Regular);
    file.set_mode(0o644);
    file.set_size(body.len() as u64);
    file.set_cksum();
    builder
        .append_data(&mut file, "nested/hello.txt", Cursor::new(body.as_slice()))
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap().to_vec()
}

fn stub_single_file_archive(body: &[u8]) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    let mut file = Header::new_gnu();
    file.set_entry_type(EntryType::Regular);
    file.set_mode(0o644);
    file.set_size(body.len() as u64);
    file.set_cksum();
    builder
        .append_data(&mut file, SINGLE_FILE_ENTRY, Cursor::new(body))
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap().to_vec()
}

fn stub_single_file_archive_bytes(body: &[u8]) -> u64 {
    let mut archive = tar::Archive::new(Cursor::new(body));
    let mut entries = archive.entries().unwrap();
    let mut entry = entries.next().unwrap().unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
    bytes.len() as u64
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
        exec_write_behavior: Arc::new(Mutex::new(exec_write_behavior)),
        exec_start_behavior: Arc::new(Mutex::new(ExecStartBehavior::Success)),
        exec_start_warnings: Arc::new(Mutex::new(Vec::new())),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
        last_transfer_import: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
        transfer_export_response: Arc::new(Mutex::new(StubTransferExportResponse::Success {
            source_type: TransferSourceType::Directory,
            compression: TransferCompression::None,
            body: stub_directory_archive(),
        })),
    }
}

pub(super) fn set_transfer_compression_support(state: &mut StubDaemonState, enabled: bool) {
    state.target_supports_transfer_compression = enabled;
}

pub(super) async fn set_transfer_export_file_response(state: &StubDaemonState, body: Vec<u8>) {
    *state.transfer_export_response.lock().await = StubTransferExportResponse::Success {
        source_type: TransferSourceType::File,
        compression: TransferCompression::None,
        body: stub_single_file_archive(&body),
    };
}

pub(super) async fn set_transfer_export_directory_response(
    state: &StubDaemonState,
    archive_body: Vec<u8>,
) {
    *state.transfer_export_response.lock().await = StubTransferExportResponse::Success {
        source_type: TransferSourceType::Directory,
        compression: TransferCompression::None,
        body: archive_body,
    };
}

pub(super) async fn spawn_stub_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::Success).await
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub(super) async fn spawn_retryable_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, ExecWriteBehavior::TemporaryFailureOnce).await
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
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
    let app = stub_router(state.clone());

    let daemon_state = remote_exec_daemon::AppState {
        config: Arc::new(remote_exec_daemon::config::DaemonConfig {
            target: state.target.clone(),
            listen: addr,
            default_workdir: PathBuf::from("."),
            transport: remote_exec_daemon::config::DaemonTransport::Tls,
            sandbox: None,
            enable_transfer_compression: state.target_supports_transfer_compression,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::Auto,
            default_shell: None,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: Some(remote_exec_daemon::config::TlsConfig {
                cert_pem: certs.daemon_cert.clone(),
                key_pem: certs.daemon_key.clone(),
                ca_pem: certs.ca_cert.clone(),
                pinned_client_cert_pem: None,
            }),
        }),
        default_shell: if cfg!(windows) {
            "cmd.exe".to_string()
        } else {
            "/bin/sh".to_string()
        },
        sandbox: None,
        supports_pty: state.target_supports_pty,
        supports_transfer_compression: state.target_supports_transfer_compression,
        windows_pty_backend_override: None,
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

pub(super) fn stub_router(state: StubDaemonState) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(exec_start))
        .route("/v1/exec/write", post(exec_write))
        .route("/v1/patch/apply", post(patch_apply))
        .route("/v1/transfer/export", post(transfer_export))
        .route("/v1/transfer/import", post(transfer_import))
        .route("/v1/image/read", post(image_read))
        .with_state(state)
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
    })
}

async fn exec_start(
    State(state): State<StubDaemonState>,
    Json(_req): Json<ExecStartRequest>,
) -> Response {
    *state.exec_start_calls.lock().await += 1;
    let behavior = *state.exec_start_behavior.lock().await;
    let warnings = state.exec_start_warnings.lock().await.clone();
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    let body = match behavior {
        ExecStartBehavior::Success => serde_json::to_value(ExecResponse {
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
        .unwrap(),
        ExecStartBehavior::RunningMissingDaemonSessionId => serde_json::json!({
            "daemon_instance_id": daemon_instance_id,
            "running": true,
            "chunk_id": "chunk-start",
            "wall_time_seconds": 0.25,
            "exit_code": null,
            "original_token_count": 1,
            "output": "ready",
            "warnings": warnings,
        }),
    };

    (StatusCode::OK, Json(body)).into_response()
}

async fn exec_write(
    State(state): State<StubDaemonState>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    assert_eq!(req.daemon_session_id, "daemon-session-1");
    let mut behavior = state.exec_write_behavior.lock().await;
    let response_behavior = *behavior;
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
        ExecWriteBehavior::MalformedCompletedMissingExitCode => {}
    }
    drop(behavior);
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    let body = match response_behavior {
        ExecWriteBehavior::MalformedCompletedMissingExitCode => serde_json::json!({
            "daemon_session_id": null,
            "daemon_instance_id": daemon_instance_id,
            "running": false,
            "chunk_id": "chunk-write",
            "wall_time_seconds": 0.5,
            "exit_code": null,
            "original_token_count": 2,
            "output": "poll output",
            "warnings": [],
        }),
        _ => serde_json::to_value(ExecResponse {
            daemon_session_id: None,
            daemon_instance_id,
            running: false,
            chunk_id: Some("chunk-write".to_string()),
            wall_time_seconds: 0.5,
            exit_code: Some(0),
            original_token_count: Some(2),
            output: "poll output".to_string(),
            warnings: Vec::new(),
        })
        .unwrap(),
    };

    Ok((StatusCode::OK, Json(body)).into_response())
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

async fn transfer_export(
    State(state): State<StubDaemonState>,
    Json(_req): Json<TransferExportRequest>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<RpcErrorBody>)> {
    match state.transfer_export_response.lock().await.clone() {
        StubTransferExportResponse::Success {
            source_type,
            compression,
            body,
        } => {
            let mut headers = HeaderMap::new();
            headers.insert(
                TRANSFER_SOURCE_TYPE_HEADER,
                HeaderValue::from_static(match source_type {
                    TransferSourceType::File => "file",
                    TransferSourceType::Directory => "directory",
                    TransferSourceType::Multiple => "multiple",
                }),
            );
            headers.insert(
                TRANSFER_COMPRESSION_HEADER,
                HeaderValue::from_static(match compression {
                    TransferCompression::None => "none",
                    TransferCompression::Zstd => "zstd",
                }),
            );
            Ok((headers, body))
        }
        StubTransferExportResponse::Error { status, body } => Err((status, Json(body))),
    }
}

async fn transfer_import(
    State(state): State<StubDaemonState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let destination_path = headers
        .get(TRANSFER_DESTINATION_PATH_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let source_type = headers
        .get(TRANSFER_SOURCE_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let compression = headers
        .get(TRANSFER_COMPRESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("none")
        .to_string();
    let overwrite = headers
        .get(TRANSFER_OVERWRITE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let create_parent = headers
        .get(TRANSFER_CREATE_PARENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    *state.last_transfer_import.lock().await = Some(StubTransferImportCapture {
        destination_path,
        source_type: source_type.clone(),
        compression: compression.clone(),
        overwrite: overwrite.clone(),
        create_parent,
        body_len: body.len(),
        body: body.to_vec(),
    });

    let parsed_source_type = match source_type.as_str() {
        "directory" => TransferSourceType::Directory,
        "multiple" => TransferSourceType::Multiple,
        _ => TransferSourceType::File,
    };
    let (bytes_copied, files_copied, directories_copied) =
        summarize_archive(&body, &parsed_source_type, &compression);

    Ok(Json(TransferImportResponse {
        source_type: parsed_source_type.clone(),
        bytes_copied,
        files_copied,
        directories_copied,
        replaced: overwrite == "replace",
    }))
}

fn summarize_archive(
    body: &[u8],
    source_type: &TransferSourceType,
    compression: &str,
) -> (u64, u64, u64) {
    let raw = match compression {
        "zstd" => zstd::stream::decode_all(Cursor::new(body)).expect("decode zstd archive"),
        _ => body.to_vec(),
    };

    match source_type {
        TransferSourceType::File => (stub_single_file_archive_bytes(&raw), 1, 0),
        TransferSourceType::Directory | TransferSourceType::Multiple => {
            let mut bytes = 0;
            let mut files = 0;
            let mut directories = matches!(
                source_type,
                TransferSourceType::Directory | TransferSourceType::Multiple
            ) as u64;
            let mut archive = tar::Archive::new(Cursor::new(raw));
            for entry in archive.entries().expect("archive entries") {
                let entry = entry.expect("archive entry");
                if entry.header().entry_type().is_dir() {
                    let path = entry.path().expect("entry path");
                    if path.as_ref() != std::path::Path::new(".") {
                        directories += 1;
                    }
                } else if entry.header().entry_type().is_file() {
                    bytes += entry.header().size().expect("entry size");
                    files += 1;
                }
            }
            (bytes, files, directories)
        }
    }
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
        .danger_accept_invalid_hostnames(true)
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
