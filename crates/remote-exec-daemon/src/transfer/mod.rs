mod codec;

pub use remote_exec_host::transfer::archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use remote_exec_host::HostRpcError;
use remote_exec_host::transfer::archive::ExportArchiveStreamItem;
use remote_exec_proto::rpc::{
    RpcErrorBody, TransferExportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse, TransferStreamDecodeError, TransferStreamExportItem,
    decode_transfer_stream_body, encode_transfer_export_item_stream,
};

use crate::AppState;
use crate::rpc_error::RpcError;
use crate::rpc_error::domain_error_response;

pub async fn path_info(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferPathInfoRequest>,
) -> Result<Json<TransferPathInfoResponse>, RpcError> {
    remote_exec_host::transfer::path_info_for_request(state.as_ref(), &req)
        .map(Json)
        .map_err(domain_error_response)
}

pub async fn export_path(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TransferExportRequest>,
) -> Result<Response, RpcError> {
    codec::require_transfer_stream_version(&headers)?;
    tracing::info!(
        path = %req.path,
        compression = codec::compression_header_value(&req.compression),
        symlink_mode = ?req.symlink_mode,
        exclude_count = req.exclude.len(),
        "transfer export received"
    );

    let exported = remote_exec_host::transfer::export_path_byte_stream_local(state, req)
        .await
        .map_err(domain_error_response)?;
    let metadata =
        codec::export_metadata(exported.source_type.clone(), exported.compression.clone());
    let body = Body::from_stream(framed_export_stream(exported.receiver));
    tracing::info!(
        source_type = codec::source_type_header_value(&metadata.source_type),
        compression = codec::compression_header_value(&metadata.compression),
        "transfer export stream started"
    );

    codec::apply_export_headers(Response::builder(), &metadata)
        .body(body)
        .map_err(|err| crate::exec::internal_error(err.into()))
}

pub async fn import_archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<TransferImportResponse>, RpcError> {
    codec::require_transfer_stream_version(&headers)?;
    codec::require_transfer_stream_content_type(&headers)?;
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
    let request = metadata.clone();
    let reader = tokio_util::io::StreamReader::new(framed_import_data_stream(body).boxed());
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

fn framed_import_data_stream(body: Body) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    decode_transfer_stream_body(http_body_util::BodyExt::into_data_stream(body))
        .map_err(decode_error_to_io_error)
}

fn decode_error_to_io_error<E>(err: TransferStreamDecodeError<E>) -> std::io::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    match err {
        TransferStreamDecodeError::Transport(err) => std::io::Error::other(err),
        TransferStreamDecodeError::Invalid(message) => {
            std::io::Error::new(std::io::ErrorKind::InvalidData, message)
        }
        TransferStreamDecodeError::MalformedComplete(err) => std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("malformed transfer stream complete frame: {err}"),
        ),
        TransferStreamDecodeError::MalformedError(err) => std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("malformed transfer stream error frame: {err}"),
        ),
        TransferStreamDecodeError::ErrorFrame { code, message } => {
            std::io::Error::other(format!("transfer stream error {code}: {message}"))
        }
    }
}

fn framed_export_stream(
    receiver: tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>,
) -> impl Stream<Item = Result<Bytes, std::convert::Infallible>> {
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver
            .recv()
            .await
            .map(|item| (transfer_export_item(item), receiver))
    });
    encode_transfer_export_item_stream(stream, transfer_error_body)
}

fn transfer_export_item(
    item: ExportArchiveStreamItem,
) -> TransferStreamExportItem<remote_exec_host::TransferError> {
    match item {
        ExportArchiveStreamItem::Data(bytes) => TransferStreamExportItem::Data(bytes),
        ExportArchiveStreamItem::Complete { archive_bytes } => {
            TransferStreamExportItem::Complete { archive_bytes }
        }
        ExportArchiveStreamItem::Error(err) => TransferStreamExportItem::Error(err),
    }
}

fn transfer_error_body(err: remote_exec_host::TransferError) -> RpcErrorBody {
    let host_error: HostRpcError = err.into();
    let (_, body) = host_error.into_rpc_parts();
    body
}
