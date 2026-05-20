use remote_exec_proto::rpc::{
    TRANSFER_STREAM_CONTENT_TYPE, TRANSFER_STREAM_PROTOCOL_VERSION, TRANSFER_STREAM_VERSION_HEADER,
    TransferExportMetadata, TransferHeaderError, TransferImportMetadata,
    parse_transfer_export_metadata_from_lookup, transfer_import_header_pairs,
};
use remote_exec_proto::transfer::TransferCompression;
use reqwest::header::CONTENT_TYPE;

use crate::daemon_client::DaemonClientError;

pub(crate) fn parse_export_metadata(
    headers: &reqwest::header::HeaderMap,
) -> Result<TransferExportMetadata, DaemonClientError> {
    require_transfer_stream_headers(headers)?;
    parse_transfer_export_metadata_from_lookup(|name| reqwest_header_string(headers, name))
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

pub(crate) fn apply_import_headers(
    builder: reqwest::RequestBuilder,
    metadata: &TransferImportMetadata,
) -> reqwest::RequestBuilder {
    let builder = builder
        .header(CONTENT_TYPE, TRANSFER_STREAM_CONTENT_TYPE)
        .header(
            TRANSFER_STREAM_VERSION_HEADER,
            TRANSFER_STREAM_PROTOCOL_VERSION.to_string(),
        );
    transfer_import_header_pairs(metadata)
        .into_iter()
        .fold(builder, |builder, (name, value)| {
            builder.header(name, value)
        })
}

pub(crate) fn compression_header_value(compression: &TransferCompression) -> &'static str {
    compression.wire_value()
}

fn reqwest_header_string(
    headers: &reqwest::header::HeaderMap,
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

fn require_transfer_stream_headers(
    headers: &reqwest::header::HeaderMap,
) -> Result<(), DaemonClientError> {
    let expected_version = TRANSFER_STREAM_PROTOCOL_VERSION.to_string();
    match headers
        .get(TRANSFER_STREAM_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value == expected_version => {}
        Some(value) => {
            return Err(DaemonClientError::Decode(anyhow::anyhow!(
                "daemon returned unsupported transfer stream protocol version `{value}`"
            )));
        }
        None => {
            return Err(DaemonClientError::Decode(anyhow::anyhow!(
                "daemon response missing `{TRANSFER_STREAM_VERSION_HEADER}`"
            )));
        }
    }

    match headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value == TRANSFER_STREAM_CONTENT_TYPE => Ok(()),
        Some(value) => Err(DaemonClientError::Decode(anyhow::anyhow!(
            "daemon returned unsupported transfer stream content type `{value}`"
        ))),
        None => Err(DaemonClientError::Decode(anyhow::anyhow!(
            "daemon response missing `{}`",
            CONTENT_TYPE.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::{
        TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
        TRANSFER_STREAM_CONTENT_TYPE, TRANSFER_STREAM_PROTOCOL_VERSION,
        TRANSFER_STREAM_VERSION_HEADER, TRANSFER_SYMLINK_MODE_HEADER, TransferImportMetadata,
        TransferOverwrite, TransferSourceType, TransferSymlinkMode,
        transfer_destination_path_header_value,
    };
    use remote_exec_proto::transfer::TransferCompression;
    use reqwest::header::CONTENT_TYPE;

    use super::*;

    fn transfer_stream_export_headers() -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(CONTENT_TYPE, TRANSFER_STREAM_CONTENT_TYPE.parse().unwrap());
        headers.insert(
            TRANSFER_STREAM_VERSION_HEADER,
            TRANSFER_STREAM_PROTOCOL_VERSION
                .to_string()
                .parse()
                .unwrap(),
        );
        headers
    }

    #[test]
    fn transfer_codec_parses_export_metadata_from_reqwest_headers() {
        let mut headers = transfer_stream_export_headers();
        headers.insert(TRANSFER_SOURCE_TYPE_HEADER, "directory".parse().unwrap());

        let parsed = parse_export_metadata(&headers).unwrap();

        assert_eq!(parsed.source_type, TransferSourceType::Directory);
        assert_eq!(parsed.compression, TransferCompression::None);
    }

    #[test]
    fn transfer_codec_rejects_invalid_export_source_type_as_decode_error() {
        let mut headers = transfer_stream_export_headers();
        headers.insert(TRANSFER_SOURCE_TYPE_HEADER, "folder".parse().unwrap());

        let err = parse_export_metadata(&headers).unwrap_err();

        assert!(matches!(err, DaemonClientError::Decode(_)));
        assert!(err.to_string().contains(TRANSFER_SOURCE_TYPE_HEADER));
    }

    #[tokio::test]
    async fn transfer_codec_applies_canonical_import_headers() {
        crate::install_crypto_provider().unwrap();
        let client = reqwest::Client::new();
        let request = apply_import_headers(
            client.post("http://127.0.0.1/v1/transfer/import"),
            &TransferImportMetadata {
                destination_path: "/tmp/out".to_string(),
                overwrite: TransferOverwrite::Replace,
                create_parent: false,
                source_type: TransferSourceType::Multiple,
                compression: TransferCompression::Zstd,
                symlink_mode: TransferSymlinkMode::Skip,
            },
        )
        .body(reqwest::Body::from(Vec::new()))
        .build()
        .unwrap();

        assert_eq!(
            request.headers()[TRANSFER_DESTINATION_PATH_HEADER],
            transfer_destination_path_header_value("/tmp/out")
        );
        assert_eq!(request.headers()[TRANSFER_OVERWRITE_HEADER], "replace");
        assert_eq!(request.headers()[TRANSFER_CREATE_PARENT_HEADER], "false");
        assert_eq!(request.headers()[TRANSFER_SOURCE_TYPE_HEADER], "multiple");
        assert_eq!(request.headers()[TRANSFER_COMPRESSION_HEADER], "zstd");
        assert_eq!(request.headers()[TRANSFER_SYMLINK_MODE_HEADER], "skip");
    }

    #[tokio::test]
    async fn transfer_codec_builds_unicode_import_destination_header() {
        crate::install_crypto_provider().unwrap();
        let client = reqwest::Client::new();
        let request = apply_import_headers(
            client.post("http://127.0.0.1/v1/transfer/import"),
            &TransferImportMetadata {
                destination_path: "/tmp/测试/привет/résumé.txt".to_string(),
                overwrite: TransferOverwrite::Replace,
                create_parent: false,
                source_type: TransferSourceType::Multiple,
                compression: TransferCompression::None,
                symlink_mode: TransferSymlinkMode::Preserve,
            },
        )
        .body(reqwest::Body::from(Vec::new()))
        .build()
        .unwrap();

        assert_ne!(
            request.headers()[TRANSFER_DESTINATION_PATH_HEADER],
            "/tmp/测试/привет/résumé.txt"
        );
    }
}
