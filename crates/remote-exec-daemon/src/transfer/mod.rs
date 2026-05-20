mod codec;

pub use remote_exec_host::transfer::archive;

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStream, TryStreamExt};
use remote_exec_host::HostRpcError;
use remote_exec_host::transfer::archive::ExportArchiveStreamItem;
use remote_exec_proto::rpc::{
    RpcErrorBody, TRANSFER_STREAM_FRAME_HEADER_LEN, TRANSFER_STREAM_PREFACE, TransferExportRequest,
    TransferImportResponse, TransferPathInfoRequest, TransferPathInfoResponse,
    TransferStreamFrameType, decode_transfer_stream_frame_header,
    encode_transfer_stream_complete_frame, encode_transfer_stream_data_frame,
    encode_transfer_stream_frame, parse_transfer_stream_complete_payload,
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
    let decoder = TransferStreamImportDecoder::new(http_body_util::BodyExt::into_data_stream(body));
    futures_util::stream::try_unfold(decoder, |mut decoder| async move {
        match decoder.next_data_frame().await? {
            Some(bytes) => Ok(Some((bytes, decoder))),
            None => Ok(None),
        }
    })
}

struct TransferStreamImportDecoder<S> {
    stream: S,
    buffer: Vec<u8>,
    offset: usize,
    preface_read: bool,
}

impl<S> TransferStreamImportDecoder<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            offset: 0,
            preface_read: false,
        }
    }
}

impl<S, E> TransferStreamImportDecoder<S>
where
    S: TryStream<Ok = Bytes, Error = E> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    async fn next_data_frame(&mut self) -> Result<Option<Bytes>, std::io::Error> {
        self.read_preface().await?;

        loop {
            let header_bytes = self
                .read_exact(
                    TRANSFER_STREAM_FRAME_HEADER_LEN,
                    "transfer stream frame header",
                )
                .await?;
            let header_array: [u8; TRANSFER_STREAM_FRAME_HEADER_LEN] = header_bytes
                .try_into()
                .expect("read_exact returned requested length");
            let header = decode_transfer_stream_frame_header(header_array)
                .map_err(invalid_transfer_stream)?;
            let payload = self
                .read_exact(header.payload_len as usize, "transfer stream frame payload")
                .await?;

            match header.frame_type {
                TransferStreamFrameType::Data if payload.is_empty() => continue,
                TransferStreamFrameType::Data => return Ok(Some(Bytes::from(payload))),
                TransferStreamFrameType::Complete => {
                    parse_complete_payload(&payload)?;
                    return Ok(None);
                }
                TransferStreamFrameType::Error => return Err(parse_error_payload(&payload)),
            }
        }
    }

    async fn read_preface(&mut self) -> Result<(), std::io::Error> {
        if self.preface_read {
            return Ok(());
        }
        let preface = self
            .read_exact(TRANSFER_STREAM_PREFACE.len(), "transfer stream preface")
            .await?;
        if preface.as_slice() != TRANSFER_STREAM_PREFACE {
            return Err(invalid_transfer_stream("invalid transfer stream preface"));
        }
        self.preface_read = true;
        Ok(())
    }

    async fn read_exact(
        &mut self,
        len: usize,
        label: &'static str,
    ) -> Result<Vec<u8>, std::io::Error> {
        while self.available() < len {
            match self.stream.try_next().await {
                Ok(Some(chunk)) if !chunk.is_empty() => self.buffer.extend_from_slice(&chunk),
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!("transfer stream ended before {label}"),
                    ));
                }
                Err(err) => return Err(std::io::Error::other(err)),
            }
        }

        let start = self.offset;
        let end = start + len;
        let output = self.buffer[start..end].to_vec();
        self.offset = end;
        self.compact_buffer();
        Ok(output)
    }

    fn available(&self) -> usize {
        self.buffer.len().saturating_sub(self.offset)
    }

    fn compact_buffer(&mut self) {
        if self.offset == 0 {
            return;
        }
        if self.offset == self.buffer.len() {
            self.buffer.clear();
            self.offset = 0;
            return;
        }
        if self.offset >= 64 * 1024 {
            self.buffer.drain(..self.offset);
            self.offset = 0;
        }
    }
}

fn parse_complete_payload(payload: &[u8]) -> Result<(), std::io::Error> {
    parse_transfer_stream_complete_payload(payload)
        .map(|_| ())
        .map_err(|err| {
            invalid_transfer_stream(format!("malformed transfer stream complete frame: {err}"))
        })
}

fn parse_error_payload(payload: &[u8]) -> std::io::Error {
    match serde_json::from_slice::<RpcErrorBody>(payload) {
        Ok(error) => std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "transfer stream error {}: {}",
                error.wire_code(),
                error.message
            ),
        ),
        Err(err) => {
            invalid_transfer_stream(format!("malformed transfer stream error frame: {err}"))
        }
    }
}

fn invalid_transfer_stream(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
}

fn framed_export_stream(
    receiver: tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>,
) -> impl Stream<Item = Result<Bytes, std::convert::Infallible>> {
    enum State {
        Preface(tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>),
        Items(tokio::sync::mpsc::Receiver<ExportArchiveStreamItem>),
        Done,
    }

    futures_util::stream::unfold(State::Preface(receiver), |state| async move {
        match state {
            State::Preface(receiver) => Some((
                Ok(Bytes::copy_from_slice(TRANSFER_STREAM_PREFACE)),
                State::Items(receiver),
            )),
            State::Items(mut receiver) => match receiver.recv().await {
                Some(ExportArchiveStreamItem::Data(bytes)) => {
                    Some((Ok(data_frame(bytes)), State::Items(receiver)))
                }
                Some(ExportArchiveStreamItem::Complete { archive_bytes }) => {
                    Some((Ok(complete_frame(archive_bytes)), State::Done))
                }
                Some(ExportArchiveStreamItem::Error(err)) => {
                    Some((Ok(error_frame(transfer_error_body(err))), State::Done))
                }
                None => Some((
                    Ok(error_frame(RpcErrorBody::new(
                        remote_exec_proto::rpc::RpcErrorCode::Internal,
                        "transfer export stream ended before terminal state",
                    ))),
                    State::Done,
                )),
            },
            State::Done => None,
        }
    })
}

fn data_frame(bytes: Bytes) -> Bytes {
    Bytes::from(encode_transfer_stream_data_frame(&bytes))
}

fn complete_frame(archive_bytes: u64) -> Bytes {
    Bytes::from(encode_transfer_stream_complete_frame(archive_bytes))
}

fn error_frame(error: RpcErrorBody) -> Bytes {
    let payload = serde_json::to_vec(&error).expect("transfer error payload serializes");
    Bytes::from(encode_transfer_stream_frame(
        TransferStreamFrameType::Error,
        &payload,
    ))
}

fn transfer_error_body(err: remote_exec_host::TransferError) -> RpcErrorBody {
    let host_error: HostRpcError = err.into();
    let (_, body) = host_error.into_rpc_parts();
    body
}
