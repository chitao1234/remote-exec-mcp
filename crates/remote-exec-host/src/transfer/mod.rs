pub mod archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::Response;
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use remote_exec_proto::rpc::{
    RpcErrorBody, TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TRANSFER_SYMLINK_MODE_HEADER, TransferCompression, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};

use crate::AppState;
use crate::error::TransferError;

pub async fn path_info(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferPathInfoRequest>,
) -> Result<Json<TransferPathInfoResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let info = path_info_for_request(state.as_ref(), &req).map_err(TransferError::into_rpc)?;
    Ok(Json(info))
}

pub fn path_info_for_request(
    state: &AppState,
    req: &TransferPathInfoRequest,
) -> Result<TransferPathInfoResponse, TransferError> {
    if !crate::host_path::is_input_path_absolute(
        &req.path,
        state.config.windows_posix_root.as_deref(),
    ) {
        return Err(TransferError::path_not_absolute(format!(
            "transfer endpoint path `{}` is not absolute",
            req.path
        )));
    }

    let path = archive::host_path(&req.path, state.config.windows_posix_root.as_deref())
        .map_err(classify_transfer_error)?;
    remote_exec_proto::sandbox::authorize_path(
        archive::host_policy(),
        state.sandbox.as_ref(),
        remote_exec_proto::sandbox::SandboxAccess::Write,
        &path,
    )
    .map_err(|err| TransferError::sandbox_denied(err.to_string()))?;

    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(TransferError::destination_unsupported(format!(
                    "destination path contains unsupported symlink `{}`",
                    path.display()
                )));
            }
            Ok(TransferPathInfoResponse {
                exists: true,
                is_directory: metadata.is_dir(),
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TransferPathInfoResponse {
            exists: false,
            is_directory: false,
        }),
        Err(err) => Err(TransferError::internal(err.to_string())),
    }
}

pub async fn export_path_local(
    state: Arc<AppState>,
    req: TransferExportRequest,
) -> Result<archive::ExportedArchiveStream, TransferError> {
    ensure_transfer_compression_supported(state.as_ref(), &req.compression)?;
    archive::export_path_to_stream(
        &req.path,
        req.compression,
        req.symlink_mode,
        &req.exclude,
        state.sandbox.as_ref(),
        state.config.windows_posix_root.as_deref(),
    )
    .await
    .map_err(classify_transfer_error)
}

pub async fn export_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferExportRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    let path = req.path.clone();
    tracing::info!(
        path = %path,
        compression = format_compression(&req.compression),
        symlink_mode = ?req.symlink_mode,
        exclude_count = req.exclude.len(),
        "transfer export received"
    );
    let exported = export_path_local(state.clone(), req)
        .await
        .map_err(TransferError::into_rpc)?;

    let stream = tokio_util::io::ReaderStream::new(exported.reader);
    let body = Body::from_stream(stream);
    tracing::info!(
        path = %path,
        source_type = format_source_type(&exported.source_type),
        compression = format_compression(&exported.compression),
        "transfer export completed"
    );

    Response::builder()
        .header(
            TRANSFER_SOURCE_TYPE_HEADER,
            format_source_type(&exported.source_type),
        )
        .header(
            TRANSFER_COMPRESSION_HEADER,
            format_compression(&exported.compression),
        )
        .body(body)
        .map_err(|err| crate::exec::internal_error(err.into()))
}

pub async fn import_archive_local(
    state: Arc<AppState>,
    request: TransferImportRequest,
    body: Body,
) -> Result<TransferImportResponse, TransferError> {
    ensure_transfer_compression_supported(state.as_ref(), &request.compression)?;
    let stream = tokio_util::io::StreamReader::new(
        BodyExt::into_data_stream(body).map_err(std::io::Error::other),
    );
    archive::import_archive_from_async_reader(
        stream,
        &request,
        state.sandbox.as_ref(),
        state.config.windows_posix_root.as_deref(),
    )
    .await
    .map_err(classify_transfer_error)
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
        source_type = format_source_type(&request.source_type),
        compression = format_compression(&request.compression),
        symlink_mode = ?request.symlink_mode,
        "transfer import received"
    );
    let summary = import_archive_local(state.clone(), request.clone(), body)
        .await
        .map_err(TransferError::into_rpc)?;
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

fn format_compression(compression: &TransferCompression) -> &'static str {
    match compression {
        TransferCompression::None => "none",
        TransferCompression::Zstd => "zstd",
    }
}

pub fn classify_transfer_error(err: anyhow::Error) -> TransferError {
    match err.downcast::<TransferError>() {
        Ok(err) => err,
        Err(err) => TransferError::internal(err.to_string()),
    }
}

pub fn map_transfer_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    classify_transfer_error(err).into_rpc()
}

pub fn parse_import_request(
    headers: &HeaderMap,
) -> Result<TransferImportRequest, (StatusCode, Json<RpcErrorBody>)> {
    Ok(TransferImportRequest {
        destination_path: header_string(headers, TRANSFER_DESTINATION_PATH_HEADER)?,
        overwrite: parse_header_enum(headers, TRANSFER_OVERWRITE_HEADER)?,
        create_parent: header_string(headers, TRANSFER_CREATE_PARENT_HEADER)?
            .parse::<bool>()
            .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))?,
        source_type: parse_header_enum(headers, TRANSFER_SOURCE_TYPE_HEADER)?,
        compression: parse_optional_header_enum(headers, TRANSFER_COMPRESSION_HEADER)?
            .unwrap_or_default(),
        symlink_mode: parse_optional_header_enum(headers, TRANSFER_SYMLINK_MODE_HEADER)?
            .unwrap_or_default(),
    })
}

fn header_string(
    headers: &HeaderMap,
    name: &str,
) -> Result<String, (StatusCode, Json<RpcErrorBody>)> {
    optional_header_string(headers, name)?.ok_or_else(|| {
        crate::exec::rpc_error("transfer_failed", format!("missing header `{name}`"))
    })
}

fn optional_header_string(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<String>, (StatusCode, Json<RpcErrorBody>)> {
    Ok(headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string))
}

fn parse_header_enum<T>(
    headers: &HeaderMap,
    name: &str,
) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    parse_header_enum_value(&header_string(headers, name)?)
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
        .map(parse_header_enum_value)
        .transpose()
}

fn parse_header_enum_value<T>(raw: &str) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))
}

fn ensure_transfer_compression_supported(
    state: &AppState,
    compression: &TransferCompression,
) -> Result<(), TransferError> {
    if matches!(compression, TransferCompression::Zstd) && !state.supports_transfer_compression {
        return Err(TransferError::compression_unsupported(
            "this daemon does not support transfer compression",
        ));
    }
    Ok(())
}
