use remote_exec_proto::rpc::{
    TransferCompression, TransferExportMetadata, TransferHeaderError, TransferImportMetadata,
    parse_transfer_export_metadata, transfer_import_header_pairs,
};

use crate::daemon_client::DaemonClientError;

pub(crate) fn parse_export_metadata(
    headers: &reqwest::header::HeaderMap,
) -> Result<TransferExportMetadata, DaemonClientError> {
    parse_transfer_export_metadata(|name| reqwest_header_string(headers, name))
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

pub(crate) fn apply_import_headers(
    builder: reqwest::RequestBuilder,
    metadata: &TransferImportMetadata,
) -> reqwest::RequestBuilder {
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
        crate::install_crypto_provider().unwrap();
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

        assert_eq!(
            request.headers()[TRANSFER_DESTINATION_PATH_HEADER],
            "/tmp/out"
        );
        assert_eq!(request.headers()[TRANSFER_OVERWRITE_HEADER], "replace");
        assert_eq!(request.headers()[TRANSFER_CREATE_PARENT_HEADER], "false");
        assert_eq!(request.headers()[TRANSFER_SOURCE_TYPE_HEADER], "multiple");
        assert_eq!(request.headers()[TRANSFER_COMPRESSION_HEADER], "zstd");
        assert_eq!(request.headers()[TRANSFER_SYMLINK_MODE_HEADER], "skip");
    }
}
