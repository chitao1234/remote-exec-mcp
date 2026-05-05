use remote_exec_proto::rpc::{
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER, TRANSFER_SYMLINK_MODE_HEADER,
    TransferCompression, TransferExportMetadata, TransferImportMetadata, TransferOverwriteMode,
    TransferSourceType, TransferSymlinkMode,
};

use crate::daemon_client::DaemonClientError;

pub(crate) fn parse_export_metadata(
    headers: &reqwest::header::HeaderMap,
) -> Result<TransferExportMetadata, DaemonClientError> {
    Ok(TransferExportMetadata {
        source_type: parse_header_enum(headers, TRANSFER_SOURCE_TYPE_HEADER)?,
        compression: parse_optional_header_enum(headers, TRANSFER_COMPRESSION_HEADER)?
            .unwrap_or_default(),
    })
}

pub(crate) fn apply_import_headers(
    builder: reqwest::RequestBuilder,
    metadata: &TransferImportMetadata,
) -> reqwest::RequestBuilder {
    builder
        .header(
            TRANSFER_DESTINATION_PATH_HEADER,
            metadata.destination_path.clone(),
        )
        .header(
            TRANSFER_OVERWRITE_HEADER,
            overwrite_header_value(&metadata.overwrite),
        )
        .header(
            TRANSFER_CREATE_PARENT_HEADER,
            metadata.create_parent.to_string(),
        )
        .header(
            TRANSFER_SOURCE_TYPE_HEADER,
            source_type_header_value(&metadata.source_type),
        )
        .header(
            TRANSFER_COMPRESSION_HEADER,
            compression_header_value(&metadata.compression),
        )
        .header(
            TRANSFER_SYMLINK_MODE_HEADER,
            symlink_mode_header_value(&metadata.symlink_mode),
        )
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

fn overwrite_header_value(overwrite: &TransferOverwriteMode) -> &'static str {
    match overwrite {
        TransferOverwriteMode::Fail => "fail",
        TransferOverwriteMode::Merge => "merge",
        TransferOverwriteMode::Replace => "replace",
    }
}

fn symlink_mode_header_value(mode: &TransferSymlinkMode) -> &'static str {
    match mode {
        TransferSymlinkMode::Preserve => "preserve",
        TransferSymlinkMode::Follow => "follow",
        TransferSymlinkMode::Skip => "skip",
    }
}

fn parse_header_enum<T>(
    headers: &reqwest::header::HeaderMap,
    name: &str,
) -> Result<T, DaemonClientError>
where
    T: serde::de::DeserializeOwned,
{
    parse_header_enum_value(header_str(headers, name)?)
}

fn parse_optional_header_enum<T>(
    headers: &reqwest::header::HeaderMap,
    name: &str,
) -> Result<Option<T>, DaemonClientError>
where
    T: serde::de::DeserializeOwned,
{
    optional_header_str(headers, name)
        .map(parse_header_enum_value)
        .transpose()
}

fn header_str<'a>(
    headers: &'a reqwest::header::HeaderMap,
    name: &str,
) -> Result<&'a str, DaemonClientError> {
    optional_header_str(headers, name)
        .ok_or_else(|| DaemonClientError::Decode(anyhow::anyhow!("missing header `{name}`")))
}

fn optional_header_str<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn parse_header_enum_value<T>(raw: &str) -> Result<T, DaemonClientError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::{
        TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
        TRANSFER_SYMLINK_MODE_HEADER, TransferCompression, TransferImportMetadata,
        TransferOverwriteMode, TransferSourceType, TransferSymlinkMode,
    };

    use super::*;

    #[test]
    fn transfer_codec_parses_export_metadata_from_reqwest_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(TRANSFER_SOURCE_TYPE_HEADER, "directory".parse().unwrap());

        let parsed = parse_export_metadata(&headers).unwrap();

        assert_eq!(parsed.source_type, TransferSourceType::Directory);
        assert_eq!(parsed.compression, TransferCompression::None);
    }

    #[test]
    fn transfer_codec_rejects_invalid_export_source_type_as_decode_error() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(TRANSFER_SOURCE_TYPE_HEADER, "folder".parse().unwrap());

        let err = parse_export_metadata(&headers).unwrap_err();

        assert!(matches!(err, DaemonClientError::Decode(_)));
        assert!(err.to_string().contains(TRANSFER_SOURCE_TYPE_HEADER));
    }

    #[tokio::test]
    async fn transfer_codec_applies_canonical_import_headers() {
        crate::install_crypto_provider();
        let client = reqwest::Client::new();
        let request = apply_import_headers(
            client.post("http://127.0.0.1/v1/transfer/import"),
            &TransferImportMetadata {
                destination_path: "/tmp/out".to_string(),
                overwrite: TransferOverwriteMode::Replace,
                create_parent: false,
                source_type: TransferSourceType::Multiple,
                compression: TransferCompression::Zstd,
                symlink_mode: TransferSymlinkMode::Skip,
            },
        )
        .body(reqwest::Body::from(Vec::new()))
        .build()
        .unwrap();

        assert_eq!(request.headers()[TRANSFER_DESTINATION_PATH_HEADER], "/tmp/out");
        assert_eq!(request.headers()[TRANSFER_OVERWRITE_HEADER], "replace");
        assert_eq!(request.headers()[TRANSFER_CREATE_PARENT_HEADER], "false");
        assert_eq!(request.headers()[TRANSFER_SOURCE_TYPE_HEADER], "multiple");
        assert_eq!(request.headers()[TRANSFER_COMPRESSION_HEADER], "zstd");
        assert_eq!(request.headers()[TRANSFER_SYMLINK_MODE_HEADER], "skip");
    }
}
