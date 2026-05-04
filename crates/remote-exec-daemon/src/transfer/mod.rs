pub use remote_exec_host::transfer::archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use remote_exec_host::HostRpcError;
use remote_exec_proto::rpc::{
    RpcErrorBody, TransferExportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};

use crate::AppState;

pub async fn path_info(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferPathInfoRequest>,
) -> Result<Json<TransferPathInfoResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::transfer::path_info_for_request(state.as_ref(), &req)
        .map(Json)
        .map_err(|err| host_rpc_error_response(err.into_host_rpc_error()))
}

pub async fn export_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferExportRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    tracing::info!(
        path = %req.path,
        compression = format_compression(&req.compression),
        symlink_mode = ?req.symlink_mode,
        exclude_count = req.exclude.len(),
        "transfer export received"
    );

    let exported = remote_exec_host::transfer::export_path_local(state, req)
        .await
        .map_err(|err| host_rpc_error_response(err.into_host_rpc_error()))?;
    let stream = tokio_util::io::ReaderStream::new(exported.reader);
    let body = Body::from_stream(stream);
    tracing::info!(
        source_type = format_source_type(&exported.source_type),
        compression = format_compression(&exported.compression),
        "transfer export completed"
    );

    Response::builder()
        .header(
            remote_exec_proto::rpc::TRANSFER_SOURCE_TYPE_HEADER,
            format_source_type(&exported.source_type),
        )
        .header(
            remote_exec_proto::rpc::TRANSFER_COMPRESSION_HEADER,
            format_compression(&exported.compression),
        )
        .body(body)
        .map_err(|err| crate::exec::internal_error(err.into()))
}

pub async fn import_archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let request = remote_exec_host::transfer::parse_import_request(&headers)?;
    tracing::info!(
        destination_path = %request.destination_path,
        overwrite = ?request.overwrite,
        create_parent = request.create_parent,
        source_type = ?request.source_type,
        compression = ?request.compression,
        symlink_mode = ?request.symlink_mode,
        "transfer import received"
    );
    let summary = remote_exec_host::transfer::import_archive_local(state, request.clone(), body)
        .await
        .map_err(|err| host_rpc_error_response(err.into_host_rpc_error()))?;
    tracing::info!(
        destination_path = %request.destination_path,
        bytes_copied = summary.bytes_copied,
        files_copied = summary.files_copied,
        directories_copied = summary.directories_copied,
        replaced = summary.replaced,
        warnings = summary.warnings.len(),
        "transfer import completed"
    );
    Ok(Json(summary))
}

fn format_source_type(source_type: &remote_exec_proto::rpc::TransferSourceType) -> &'static str {
    match source_type {
        remote_exec_proto::rpc::TransferSourceType::File => "file",
        remote_exec_proto::rpc::TransferSourceType::Directory => "directory",
        remote_exec_proto::rpc::TransferSourceType::Multiple => "multiple",
    }
}

fn format_compression(compression: &remote_exec_proto::rpc::TransferCompression) -> &'static str {
    match compression {
        remote_exec_proto::rpc::TransferCompression::None => "none",
        remote_exec_proto::rpc::TransferCompression::Zstd => "zstd",
    }
}

fn host_rpc_error_response(err: HostRpcError) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::from_u16(err.status).expect("valid host rpc status"),
        Json(RpcErrorBody {
            code: err.code.to_string(),
            message: err.message,
        }),
    )
}
