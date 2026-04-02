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
    RpcErrorBody, TransferExportRequest, TransferImportRequest, TransferImportResponse,
    TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER,
    TRANSFER_SOURCE_TYPE_HEADER,
};

use crate::AppState;

pub async fn export_path(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<TransferExportRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    let exported = archive::export_path_to_archive(std::path::Path::new(&req.path))
        .await
        .map_err(map_transfer_error)?;

    let file = tokio::fs::File::open(exported.temp_path.to_path_buf())
        .await
        .map_err(|err| crate::exec::internal_error(err.into()))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok((
        [(
            TRANSFER_SOURCE_TYPE_HEADER,
            format_source_type(&exported.source_type).to_string(),
        )],
        body,
    )
        .into_response())
}

pub async fn import_archive(
    headers: HeaderMap,
    body: Body,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let request = parse_import_request(&headers)?;
    let temp = tempfile::NamedTempFile::new().map_err(|err| crate::exec::internal_error(err.into()))?;
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

    let summary = archive::import_archive_from_file(&temp_path, &request)
        .await
        .map_err(map_transfer_error)?;
    Ok(Json(summary))
}

fn format_source_type(source_type: &remote_exec_proto::rpc::TransferSourceType) -> &'static str {
    match source_type {
        remote_exec_proto::rpc::TransferSourceType::File => "file",
        remote_exec_proto::rpc::TransferSourceType::Directory => "directory",
    }
}

fn map_transfer_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    let message = err.to_string();
    let code = if message.contains("not absolute") {
        "transfer_path_not_absolute"
    } else if message.contains("destination path") && message.contains("already exists") {
        "transfer_destination_exists"
    } else if message.contains("destination parent") && message.contains("does not exist") {
        "transfer_parent_missing"
    } else if message.contains("unsupported symlink")
        || message.contains("unsupported entry")
        || message.contains("regular file or directory")
    {
        "transfer_source_unsupported"
    } else if message.contains("No such file or directory") {
        "transfer_source_missing"
    } else {
        "transfer_failed"
    };

    crate::exec::rpc_error(code, message)
}

fn parse_import_request(headers: &HeaderMap) -> Result<TransferImportRequest, (StatusCode, Json<RpcErrorBody>)> {
    Ok(TransferImportRequest {
        destination_path: header_string(headers, TRANSFER_DESTINATION_PATH_HEADER)?,
        overwrite: parse_header_enum(headers, TRANSFER_OVERWRITE_HEADER)?,
        create_parent: header_string(headers, TRANSFER_CREATE_PARENT_HEADER)?
            .parse::<bool>()
            .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))?,
        source_type: parse_header_enum(headers, TRANSFER_SOURCE_TYPE_HEADER)?,
    })
}

fn header_string(headers: &HeaderMap, name: &str) -> Result<String, (StatusCode, Json<RpcErrorBody>)> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| crate::exec::rpc_error("transfer_failed", format!("missing header `{name}`")))
}

fn parse_header_enum<T>(headers: &HeaderMap, name: &str) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    let raw = header_string(headers, name)?;
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))
}
