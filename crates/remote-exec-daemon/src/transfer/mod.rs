pub mod archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use remote_exec_proto::rpc::{
    RpcErrorBody, TransferExportRequest, TransferImportResponse, TRANSFER_SOURCE_TYPE_HEADER,
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

pub async fn import_archive() -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    Err(crate::exec::rpc_error(
        "transfer_failed",
        "transfer import not implemented",
    ))
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
