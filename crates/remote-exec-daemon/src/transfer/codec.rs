use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use remote_exec_proto::rpc::{
    TRANSFER_STREAM_CONTENT_TYPE, TRANSFER_STREAM_PROTOCOL_VERSION, TRANSFER_STREAM_VERSION_HEADER,
    TransferExportMetadata, TransferHeaderError, TransferImportMetadata, TransferSourceType,
    parse_transfer_import_metadata_from_lookup, transfer_export_header_pairs,
};
use remote_exec_proto::transfer::TransferCompression;

use crate::rpc_error::RpcError;
use crate::rpc_error::bad_request;

pub(crate) fn export_metadata(
    source_type: TransferSourceType,
    compression: TransferCompression,
) -> TransferExportMetadata {
    TransferExportMetadata {
        source_type,
        compression,
    }
}

pub(crate) fn apply_export_headers(
    builder: axum::http::response::Builder,
    metadata: &TransferExportMetadata,
) -> axum::http::response::Builder {
    let builder = builder
        .header(
            axum::http::header::CONTENT_TYPE,
            TRANSFER_STREAM_CONTENT_TYPE,
        )
        .header(
            TRANSFER_STREAM_VERSION_HEADER,
            TRANSFER_STREAM_PROTOCOL_VERSION.to_string(),
        );
    transfer_export_header_pairs(metadata)
        .into_iter()
        .fold(builder, |builder, (name, value)| {
            builder.header(name, value)
        })
}

pub(crate) fn require_transfer_stream_version(headers: &HeaderMap) -> Result<(), RpcError> {
    let expected = TRANSFER_STREAM_PROTOCOL_VERSION.to_string();
    match headers
        .get(TRANSFER_STREAM_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value == expected => Ok(()),
        Some(value) => Err(bad_request(format!(
            "unsupported transfer stream protocol version `{value}`"
        ))),
        None => Err(bad_request(format!(
            "missing header `{TRANSFER_STREAM_VERSION_HEADER}`"
        ))),
    }
}

pub(crate) fn require_transfer_stream_content_type(headers: &HeaderMap) -> Result<(), RpcError> {
    match headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value == TRANSFER_STREAM_CONTENT_TYPE => Ok(()),
        Some(value) => Err(bad_request(format!(
            "unsupported transfer stream content type `{value}`"
        ))),
        None => Err(bad_request(format!(
            "missing header `{}`",
            CONTENT_TYPE.as_str()
        ))),
    }
}

pub(crate) fn parse_import_metadata(
    headers: &HeaderMap,
) -> Result<TransferImportMetadata, RpcError> {
    parse_transfer_import_metadata_from_lookup(|name| axum_header_string(headers, name))
        .map_err(|err| bad_request(err.to_string()))
}

pub(crate) fn source_type_header_value(source_type: &TransferSourceType) -> &'static str {
    source_type.wire_value()
}

pub(crate) fn compression_header_value(compression: &TransferCompression) -> &'static str {
    compression.wire_value()
}

fn axum_header_string(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, TransferHeaderError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|err| TransferHeaderError::invalid(name, err.to_string()))
        })
        .transpose()
}
