use std::io::Cursor;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use remote_exec_proto::rpc::{
    RpcErrorBody, TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TRANSFER_SYMLINK_MODE_HEADER, TransferExportRequest, TransferHeaders, TransferImportResponse,
    TransferPathInfoRequest, TransferPathInfoResponse, TransferSourceType, TransferWarning,
    parse_transfer_import_metadata,
};
use remote_exec_proto::transfer::TransferCompression;
use tar::{Builder, EntryType, Header};

use super::StubDaemonState;

const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";
const TRANSFER_SUMMARY_ENTRY: &str = ".remote-exec-transfer-summary.json";

#[derive(Debug, Clone)]
pub struct StubTransferImportCapture {
    pub destination_path: String,
    pub source_type: String,
    pub compression: String,
    pub overwrite: String,
    pub create_parent: String,
    pub symlink_mode: String,
    pub body_len: usize,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct StubTransferExportCapture {
    pub request: TransferExportRequest,
}

#[derive(Debug, Clone)]
pub(super) enum StubTransferExportResponse {
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

#[derive(Debug, Clone)]
pub enum StubTransferPathInfoResponse {
    Success(TransferPathInfoResponse),
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

pub(super) fn default_transfer_export_response() -> StubTransferExportResponse {
    StubTransferExportResponse::Success {
        source_type: TransferSourceType::Directory,
        compression: TransferCompression::None,
        body: stub_directory_archive(),
    }
}

pub(super) fn default_transfer_path_info_response() -> StubTransferPathInfoResponse {
    StubTransferPathInfoResponse::Success(TransferPathInfoResponse {
        exists: false,
        is_directory: false,
    })
}

pub(crate) async fn set_transfer_export_file_response(state: &StubDaemonState, body: Vec<u8>) {
    *state.transfer_export_response.lock().await = StubTransferExportResponse::Success {
        source_type: TransferSourceType::File,
        compression: TransferCompression::None,
        body: stub_single_file_archive(&body),
    };
}

pub(crate) async fn set_transfer_export_directory_response(
    state: &StubDaemonState,
    archive_body: Vec<u8>,
) {
    *state.transfer_export_response.lock().await = StubTransferExportResponse::Success {
        source_type: TransferSourceType::Directory,
        compression: TransferCompression::None,
        body: archive_body,
    };
}

pub(crate) async fn set_transfer_path_info_response(
    state: &StubDaemonState,
    response: TransferPathInfoResponse,
) {
    *state.transfer_path_info_response.lock().await =
        StubTransferPathInfoResponse::Success(response);
}

pub(crate) async fn set_transfer_path_info_error_response(
    state: &StubDaemonState,
    status: StatusCode,
    body: RpcErrorBody,
) {
    *state.transfer_path_info_response.lock().await =
        StubTransferPathInfoResponse::Error { status, body };
}

pub(super) async fn transfer_export(
    State(state): State<StubDaemonState>,
    Json(req): Json<TransferExportRequest>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<RpcErrorBody>)> {
    *state.last_transfer_export.lock().await = Some(StubTransferExportCapture {
        request: req.clone(),
    });
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

pub(super) async fn transfer_path_info(
    State(state): State<StubDaemonState>,
    Json(_req): Json<TransferPathInfoRequest>,
) -> Result<Json<TransferPathInfoResponse>, (StatusCode, Json<RpcErrorBody>)> {
    match state.transfer_path_info_response.lock().await.clone() {
        StubTransferPathInfoResponse::Success(response) => Ok(Json(response)),
        StubTransferPathInfoResponse::Error { status, body } => Err((status, Json(body))),
    }
}

pub(super) async fn transfer_import(
    State(state): State<StubDaemonState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let destination_path = parse_transfer_import_metadata(&TransferHeaders {
        destination_path: headers
            .get(TRANSFER_DESTINATION_PATH_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        overwrite: headers
            .get(TRANSFER_OVERWRITE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        create_parent: headers
            .get(TRANSFER_CREATE_PARENT_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        source_type: headers
            .get(TRANSFER_SOURCE_TYPE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        compression: headers
            .get(TRANSFER_COMPRESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        symlink_mode: headers
            .get(TRANSFER_SYMLINK_MODE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    })
    .map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "bad_request".to_string(),
                message: err.to_string(),
            }),
        )
    })?
    .destination_path;
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
    let symlink_mode = headers
        .get(TRANSFER_SYMLINK_MODE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    *state.last_transfer_import.lock().await = Some(StubTransferImportCapture {
        destination_path,
        source_type: source_type.clone(),
        compression: compression.clone(),
        overwrite: overwrite.clone(),
        create_parent,
        symlink_mode,
        body_len: body.len(),
        body: body.to_vec(),
    });

    let parsed_source_type = match source_type.as_str() {
        "directory" => TransferSourceType::Directory,
        "multiple" => TransferSourceType::Multiple,
        _ => TransferSourceType::File,
    };
    let (bytes_copied, files_copied, directories_copied, warnings) =
        summarize_archive(&body, &parsed_source_type, &compression);

    Ok(Json(TransferImportResponse {
        source_type: parsed_source_type.clone(),
        bytes_copied,
        files_copied,
        directories_copied,
        replaced: overwrite == "replace",
        warnings,
    }))
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

fn stub_single_file_archive_summary(body: &[u8]) -> (u64, Vec<TransferWarning>) {
    let mut archive = tar::Archive::new(Cursor::new(body));
    let mut entries = archive.entries().unwrap();
    let mut entry = entries.next().unwrap().unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
    let mut warnings = Vec::new();
    for entry in entries {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().as_ref() != std::path::Path::new(TRANSFER_SUMMARY_ENTRY) {
            continue;
        }
        warnings.extend(read_transfer_summary(&mut entry));
    }
    (bytes.len() as u64, warnings)
}

fn summarize_archive(
    body: &[u8],
    source_type: &TransferSourceType,
    compression: &str,
) -> (u64, u64, u64, Vec<TransferWarning>) {
    let raw = match compression {
        "zstd" => zstd::stream::decode_all(Cursor::new(body)).expect("decode zstd archive"),
        _ => body.to_vec(),
    };

    match source_type {
        TransferSourceType::File => {
            let (bytes, warnings) = stub_single_file_archive_summary(&raw);
            (bytes, 1, 0, warnings)
        }
        TransferSourceType::Directory | TransferSourceType::Multiple => {
            let mut bytes = 0;
            let mut files = 0;
            let mut warnings = Vec::new();
            let mut directories = matches!(
                source_type,
                TransferSourceType::Directory | TransferSourceType::Multiple
            ) as u64;
            let mut archive = tar::Archive::new(Cursor::new(raw));
            for entry in archive.entries().expect("archive entries") {
                let mut entry = entry.expect("archive entry");
                let path = entry.path().expect("entry path").into_owned();
                if path == std::path::Path::new(TRANSFER_SUMMARY_ENTRY) {
                    warnings.extend(read_transfer_summary(&mut entry));
                    continue;
                }
                if entry.header().entry_type().is_dir() {
                    if path != std::path::Path::new(".") {
                        directories += 1;
                    }
                } else if entry.header().entry_type().is_file() {
                    bytes += entry.header().size().expect("entry size");
                    files += 1;
                }
            }
            (bytes, files, directories, warnings)
        }
    }
}

fn read_transfer_summary<R: std::io::Read>(entry: &mut tar::Entry<R>) -> Vec<TransferWarning> {
    let summary: serde_json::Value = serde_json::from_reader(entry).unwrap();
    summary["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|warning| TransferWarning {
            code: warning["code"].as_str().unwrap_or_default().to_string(),
            message: warning["message"].as_str().unwrap_or_default().to_string(),
        })
        .collect()
}
