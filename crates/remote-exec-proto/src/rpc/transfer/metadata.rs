use base64::Engine;

use crate::transfer::{
    TransferCompression, TransferExportMetadata, TransferImportMetadata, TransferOverwrite,
    TransferSourceType, TransferSymlinkMode,
};

use super::headers::{
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER, TRANSFER_SYMLINK_MODE_HEADER,
    TransferHeaders,
};
use super::types::TransferHeaderError;

pub fn parse_transfer_export_metadata(
    headers: &TransferHeaders,
) -> Result<TransferExportMetadata, TransferHeaderError> {
    Ok(TransferExportMetadata {
        source_type: parse_required_source_type(headers)?,
        compression: parse_optional_compression(headers)?,
    })
}

pub fn parse_transfer_import_metadata(
    headers: &TransferHeaders,
) -> Result<TransferImportMetadata, TransferHeaderError> {
    Ok(TransferImportMetadata {
        destination_path: decode_transfer_destination_path_header(required_transfer_header(
            headers.destination_path.as_ref(),
            TRANSFER_DESTINATION_PATH_HEADER,
        )?)?,
        overwrite: parse_required_overwrite(headers)?,
        create_parent: parse_required_create_parent(headers)?,
        source_type: parse_required_source_type(headers)?,
        compression: parse_optional_compression(headers)?,
        symlink_mode: parse_optional_symlink_mode(headers)?,
    })
}

pub fn parse_transfer_export_metadata_from_lookup<F>(
    lookup: F,
) -> Result<TransferExportMetadata, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    parse_transfer_export_metadata(&TransferHeaders::from_lookup(lookup)?)
}

pub fn parse_transfer_import_metadata_from_lookup<F>(
    lookup: F,
) -> Result<TransferImportMetadata, TransferHeaderError>
where
    F: FnMut(&'static str) -> Result<Option<String>, TransferHeaderError>,
{
    parse_transfer_import_metadata(&TransferHeaders::from_lookup(lookup)?)
}

fn required_transfer_header(
    value: Option<&String>,
    name: &'static str,
) -> Result<String, TransferHeaderError> {
    let value = value
        .cloned()
        .ok_or_else(|| TransferHeaderError::missing(name))?;
    validate_transfer_header_value(&value, name)?;
    Ok(value)
}

fn optional_transfer_header(
    value: Option<&String>,
    name: &'static str,
) -> Result<Option<String>, TransferHeaderError> {
    match value {
        Some(value) => {
            validate_transfer_header_value(value, name)?;
            Ok(Some(value.clone()))
        }
        None => Ok(None),
    }
}

fn validate_transfer_header_value(
    value: &str,
    name: &'static str,
) -> Result<(), TransferHeaderError> {
    if value.contains('\n') || value.contains('\r') {
        Err(TransferHeaderError::invalid(
            name,
            "header value contains invalid newline characters",
        ))
    } else {
        Ok(())
    }
}

fn decode_transfer_destination_path_header(encoded: String) -> Result<String, TransferHeaderError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|err| {
            TransferHeaderError::invalid(
                TRANSFER_DESTINATION_PATH_HEADER,
                format!("expected base64-encoded UTF-8 path: {err}"),
            )
        })?;
    String::from_utf8(bytes).map_err(|err| {
        TransferHeaderError::invalid(
            TRANSFER_DESTINATION_PATH_HEADER,
            format!("expected base64-encoded UTF-8 path: {err}"),
        )
    })
}

fn parse_required_source_type(
    headers: &TransferHeaders,
) -> Result<TransferSourceType, TransferHeaderError> {
    let raw = required_transfer_header(headers.source_type.as_ref(), TRANSFER_SOURCE_TYPE_HEADER)?;
    TransferSourceType::from_wire_value(&raw).ok_or_else(|| {
        TransferHeaderError::invalid(
            TRANSFER_SOURCE_TYPE_HEADER,
            "expected one of `file`, `directory`, `multiple`",
        )
    })
}

fn parse_required_overwrite(
    headers: &TransferHeaders,
) -> Result<TransferOverwrite, TransferHeaderError> {
    let raw = required_transfer_header(headers.overwrite.as_ref(), TRANSFER_OVERWRITE_HEADER)?;
    TransferOverwrite::from_wire_value(&raw).ok_or_else(|| {
        TransferHeaderError::invalid(
            TRANSFER_OVERWRITE_HEADER,
            "expected one of `fail`, `merge`, `replace`",
        )
    })
}

fn parse_required_create_parent(headers: &TransferHeaders) -> Result<bool, TransferHeaderError> {
    match required_transfer_header(
        headers.create_parent.as_ref(),
        TRANSFER_CREATE_PARENT_HEADER,
    )?
    .as_str()
    {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(TransferHeaderError::invalid(
            TRANSFER_CREATE_PARENT_HEADER,
            "expected `true` or `false`",
        )),
    }
}

fn parse_optional_compression(
    headers: &TransferHeaders,
) -> Result<TransferCompression, TransferHeaderError> {
    optional_transfer_header(headers.compression.as_ref(), TRANSFER_COMPRESSION_HEADER)?
        .as_deref()
        .map(|raw| {
            TransferCompression::from_wire_value(raw).ok_or_else(|| {
                TransferHeaderError::invalid(
                    TRANSFER_COMPRESSION_HEADER,
                    "expected one of `none`, `zstd`",
                )
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse_optional_symlink_mode(
    headers: &TransferHeaders,
) -> Result<TransferSymlinkMode, TransferHeaderError> {
    optional_transfer_header(headers.symlink_mode.as_ref(), TRANSFER_SYMLINK_MODE_HEADER)?
        .as_deref()
        .map(|raw| {
            TransferSymlinkMode::from_wire_value(raw).ok_or_else(|| {
                TransferHeaderError::invalid(
                    TRANSFER_SYMLINK_MODE_HEADER,
                    "expected one of `preserve`, `follow`, `skip`",
                )
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}
