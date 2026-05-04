use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use remote_exec_proto::rpc::{
    RpcErrorBody, TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TRANSFER_SYMLINK_MODE_HEADER, TransferCompression, TransferExportMetadata,
    TransferImportMetadata, TransferSourceType,
};

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
    builder
        .header(
            TRANSFER_SOURCE_TYPE_HEADER,
            source_type_header_value(&metadata.source_type),
        )
        .header(
            TRANSFER_COMPRESSION_HEADER,
            compression_header_value(&metadata.compression),
        )
}

pub(crate) fn parse_import_metadata(
    headers: &HeaderMap,
) -> Result<TransferImportMetadata, (StatusCode, Json<RpcErrorBody>)> {
    Ok(TransferImportMetadata {
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

pub(crate) fn source_type_header_value(source_type: &TransferSourceType) -> &'static str {
    match source_type {
        TransferSourceType::File => "file",
        TransferSourceType::Directory => "directory",
        TransferSourceType::Multiple => "multiple",
    }
}

pub(crate) fn compression_header_value(compression: &TransferCompression) -> &'static str {
    match compression {
        TransferCompression::None => "none",
        TransferCompression::Zstd => "zstd",
    }
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
