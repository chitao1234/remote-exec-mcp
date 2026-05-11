use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use remote_exec_proto::rpc::{
    RpcErrorBody, TransferExportMetadata, TransferHeaderError, TransferImportMetadata,
    TransferSourceType, parse_transfer_import_metadata, transfer_export_header_pairs,
};
use remote_exec_proto::transfer::TransferCompression;

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
    transfer_export_header_pairs(metadata)
        .into_iter()
        .fold(builder, |builder, (name, value)| {
            builder.header(name, value)
        })
}

pub(crate) fn parse_import_metadata(
    headers: &HeaderMap,
) -> Result<TransferImportMetadata, (StatusCode, Json<RpcErrorBody>)> {
    parse_transfer_import_metadata(|name| axum_header_string(headers, name))
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
