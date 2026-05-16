mod codec;

pub use remote_exec_host::transfer::archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use remote_exec_proto::rpc::{
    RpcErrorBody, TransferExportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};

use crate::AppState;
use crate::rpc_error::domain_error_response;

pub async fn path_info(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferPathInfoRequest>,
) -> Result<Json<TransferPathInfoResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::transfer::path_info_for_request(state.as_ref(), &req)
        .map(Json)
        .map_err(domain_error_response)
}

pub async fn export_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferExportRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    tracing::info!(
        path = %req.path,
        compression = codec::compression_header_value(&req.compression),
        symlink_mode = ?req.symlink_mode,
        exclude_count = req.exclude.len(),
        "transfer export received"
    );

    let exported = remote_exec_host::transfer::export_path_local(state, req)
        .await
        .map_err(domain_error_response)?;
    let metadata =
        codec::export_metadata(exported.source_type.clone(), exported.compression.clone());
    let stream = tokio_util::io::ReaderStream::new(exported.reader);
    let body = Body::from_stream(stream);
    tracing::info!(
        source_type = codec::source_type_header_value(&metadata.source_type),
        compression = codec::compression_header_value(&metadata.compression),
        "transfer export completed"
    );

    codec::apply_export_headers(Response::builder(), &metadata)
        .body(body)
        .map_err(|err| crate::exec::internal_error(err.into()))
}

pub async fn import_archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let metadata = codec::parse_import_metadata(&headers)?;
    tracing::info!(
        destination_path = %metadata.destination_path,
        overwrite = ?metadata.overwrite,
        create_parent = metadata.create_parent,
        source_type = ?metadata.source_type,
        compression = ?metadata.compression,
        symlink_mode = ?metadata.symlink_mode,
        "transfer import received"
    );
    let reader = tokio_util::io::StreamReader::new(
        BodyExt::into_data_stream(body).map_err(std::io::Error::other),
    );
    let request = metadata.clone();
    let summary = remote_exec_host::transfer::import_archive_local(state, request, reader)
        .await
        .map_err(domain_error_response)?;
    tracing::info!(
        destination_path = %metadata.destination_path,
        bytes_copied = summary.bytes_copied,
        files_copied = summary.files_copied,
        directories_copied = summary.directories_copied,
        replaced = summary.replaced,
        warnings = summary.warnings.len(),
        "transfer import completed"
    );
    Ok(Json(summary))
}
