pub mod archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use remote_exec_proto::rpc::{
    RpcErrorBody, TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TransferCompression, TransferExportRequest, TransferImportRequest, TransferImportResponse,
};
use remote_exec_proto::sandbox::SandboxError;

use crate::AppState;

pub async fn export_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferExportRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    ensure_transfer_compression_supported(state.as_ref(), &req.compression)?;
    tracing::info!(
        path = %req.path,
        compression = format_compression(&req.compression),
        "transfer export received"
    );
    let exported = archive::export_path_to_archive(
        &req.path,
        req.compression.clone(),
        state.sandbox.as_ref(),
        state.config.windows_posix_root.as_deref(),
    )
    .await
    .map_err(map_transfer_error)?;

    let file = tokio::fs::File::open(exported.temp_path.to_path_buf())
        .await
        .map_err(|err| crate::exec::internal_error(err.into()))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);
    tracing::info!(
        path = %req.path,
        source_type = format_source_type(&exported.source_type),
        compression = format_compression(&exported.compression),
        "transfer export completed"
    );

    Ok((
        [
            (
                TRANSFER_SOURCE_TYPE_HEADER,
                format_source_type(&exported.source_type).to_string(),
            ),
            (
                TRANSFER_COMPRESSION_HEADER,
                format_compression(&exported.compression).to_string(),
            ),
        ],
        body,
    )
        .into_response())
}

pub async fn import_archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let request = parse_import_request(&headers)?;
    ensure_transfer_compression_supported(state.as_ref(), &request.compression)?;
    tracing::info!(
        destination_path = %request.destination_path,
        overwrite = ?request.overwrite,
        create_parent = request.create_parent,
        source_type = format_source_type(&request.source_type),
        compression = format_compression(&request.compression),
        "transfer import received"
    );
    let temp =
        tempfile::NamedTempFile::new().map_err(|err| crate::exec::internal_error(err.into()))?;
    let temp_path = temp.into_temp_path();
    let mut file = tokio::fs::File::create(temp_path.to_path_buf())
        .await
        .map_err(|err| crate::exec::internal_error(err.into()))?;
    let mut stream = tokio_util::io::StreamReader::new(
        BodyExt::into_data_stream(body).map_err(std::io::Error::other),
    );
    tokio::io::copy(&mut stream, &mut file)
        .await
        .map_err(|err| crate::exec::internal_error(err.into()))?;

    let summary = archive::import_archive_from_file(
        &temp_path,
        &request,
        state.sandbox.as_ref(),
        state.config.windows_posix_root.as_deref(),
    )
    .await
    .map_err(map_transfer_error)?;
    tracing::info!(
        destination_path = %request.destination_path,
        bytes_copied = summary.bytes_copied,
        files_copied = summary.files_copied,
        directories_copied = summary.directories_copied,
        replaced = summary.replaced,
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

fn map_transfer_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    let message = err.to_string();
    let code = if err.downcast_ref::<SandboxError>().is_some() {
        "sandbox_denied"
    } else if message.contains("not absolute") {
        "transfer_path_not_absolute"
    } else if message.contains("destination path") && message.contains("already exists") {
        "transfer_destination_exists"
    } else if message.contains("destination parent") && message.contains("does not exist") {
        "transfer_parent_missing"
    } else if message.contains("transfer compression")
        || message.contains("does not support transfer compression")
    {
        "transfer_compression_unsupported"
    } else if message.contains("unsupported symlink")
        || message.contains("unsupported entry")
        || message.contains("regular file or directory")
        || message.contains("paths in archives must not have `..`")
    {
        "transfer_source_unsupported"
    } else if message.contains("No such file or directory") {
        "transfer_source_missing"
    } else {
        "transfer_failed"
    };

    crate::exec::rpc_error(code, message)
}

fn parse_import_request(
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
    })
}

fn header_string(
    headers: &HeaderMap,
    name: &str,
) -> Result<String, (StatusCode, Json<RpcErrorBody>)> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| {
            crate::exec::rpc_error("transfer_failed", format!("missing header `{name}`"))
        })
}

fn parse_header_enum<T>(
    headers: &HeaderMap,
    name: &str,
) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    let raw = header_string(headers, name)?;
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))
}

fn parse_optional_header_enum<T>(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<T>, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    match headers.get(name).and_then(|value| value.to_str().ok()) {
        Some(raw) => serde_json::from_str::<T>(&format!("\"{raw}\""))
            .map(Some)
            .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string())),
        None => Ok(None),
    }
}

fn ensure_transfer_compression_supported(
    state: &AppState,
    compression: &TransferCompression,
) -> Result<(), (StatusCode, Json<RpcErrorBody>)> {
    if matches!(compression, TransferCompression::Zstd) && !state.supports_transfer_compression {
        return Err(crate::exec::rpc_error(
            "transfer_compression_unsupported",
            "this daemon does not support transfer compression".to_string(),
        ));
    }
    Ok(())
}
