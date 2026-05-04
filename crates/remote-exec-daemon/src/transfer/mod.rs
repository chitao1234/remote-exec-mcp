pub use remote_exec_host::transfer::archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use remote_exec_proto::rpc::{
    RpcErrorBody, TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TRANSFER_SYMLINK_MODE_HEADER, TransferExportRequest, TransferImportRequest,
    TransferImportResponse, TransferPathInfoRequest, TransferPathInfoResponse,
};

use crate::AppState;
use crate::rpc_error::{bad_request, host_rpc_error_response};

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
    let request = parse_import_request(&headers)?;
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

fn parse_import_request(
    headers: &HeaderMap,
) -> Result<TransferImportRequest, (StatusCode, Json<RpcErrorBody>)> {
    Ok(TransferImportRequest {
        destination_path: required_header_string(headers, TRANSFER_DESTINATION_PATH_HEADER)?,
        overwrite: parse_required_header_enum(headers, TRANSFER_OVERWRITE_HEADER)?,
        create_parent: required_header_string(headers, TRANSFER_CREATE_PARENT_HEADER)?
            .parse::<bool>()
            .map_err(|err| {
                bad_request(format!(
                    "invalid header `{TRANSFER_CREATE_PARENT_HEADER}`: {err}"
                ))
            })?,
        source_type: parse_required_header_enum(headers, TRANSFER_SOURCE_TYPE_HEADER)?,
        compression: parse_optional_header_enum(headers, TRANSFER_COMPRESSION_HEADER)?
            .unwrap_or_default(),
        symlink_mode: parse_optional_header_enum(headers, TRANSFER_SYMLINK_MODE_HEADER)?
            .unwrap_or_default(),
    })
}

fn required_header_string(
    headers: &HeaderMap,
    name: &str,
) -> Result<String, (StatusCode, Json<RpcErrorBody>)> {
    optional_header_string(headers, name)?
        .ok_or_else(|| bad_request(format!("missing header `{name}`")))
}

fn optional_header_string(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<String>, (StatusCode, Json<RpcErrorBody>)> {
    match headers.get(name) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .map(|value| Some(value.to_string()))
            .map_err(|err| bad_request(format!("invalid header `{name}`: {err}"))),
    }
}

fn parse_required_header_enum<T>(
    headers: &HeaderMap,
    name: &str,
) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    parse_header_enum_value(name, &required_header_string(headers, name)?)
}

fn parse_optional_header_enum<T>(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<T>, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    optional_header_string(headers, name)?
        .as_deref()
        .map(|raw| parse_header_enum_value(name, raw))
        .transpose()
}

fn parse_header_enum_value<T>(name: &str, raw: &str) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| bad_request(format!("invalid header `{name}`: {err}")))
}
