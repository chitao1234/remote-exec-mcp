mod headers;
mod metadata;
mod types;

pub use headers::{
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER, TRANSFER_SYMLINK_MODE_HEADER,
    TransferHeaderPairs, TransferHeaders, transfer_destination_path_header_value,
    transfer_export_header_pairs, transfer_import_header_pairs,
};
pub use metadata::{
    parse_transfer_export_metadata, parse_transfer_export_metadata_from_lookup,
    parse_transfer_import_metadata, parse_transfer_import_metadata_from_lookup,
};
pub use types::{
    TransferHeaderError, TransferHeaderErrorKind, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse, TransferWarning,
};

#[cfg(test)]
mod tests {
    use super::{
        TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
        TRANSFER_SYMLINK_MODE_HEADER, TransferHeaderErrorKind, TransferHeaders,
        parse_transfer_export_metadata, parse_transfer_import_metadata,
        transfer_destination_path_header_value, transfer_export_header_pairs,
        transfer_import_header_pairs,
    };
    use crate::transfer::{
        TransferCompression, TransferExportMetadata, TransferImportMetadata, TransferOverwrite,
        TransferSourceType, TransferSymlinkMode,
    };

    fn transfer_headers(headers: &[(&'static str, &'static str)]) -> TransferHeaders {
        let mut values = TransferHeaders::default();
        for (name, value) in headers {
            match *name {
                TRANSFER_DESTINATION_PATH_HEADER => {
                    values.destination_path = Some((*value).to_string())
                }
                TRANSFER_OVERWRITE_HEADER => values.overwrite = Some((*value).to_string()),
                TRANSFER_CREATE_PARENT_HEADER => values.create_parent = Some((*value).to_string()),
                TRANSFER_SOURCE_TYPE_HEADER => values.source_type = Some((*value).to_string()),
                TRANSFER_COMPRESSION_HEADER => values.compression = Some((*value).to_string()),
                TRANSFER_SYMLINK_MODE_HEADER => values.symlink_mode = Some((*value).to_string()),
                _ => panic!("unexpected transfer header name `{name}`"),
            }
        }
        values
    }

    #[test]
    fn transfer_header_pairs_render_canonical_import_metadata() {
        let metadata = TransferImportMetadata {
            destination_path: "/tmp/output".to_string(),
            overwrite: TransferOverwrite::Replace,
            create_parent: true,
            source_type: TransferSourceType::Directory,
            compression: TransferCompression::Zstd,
            symlink_mode: TransferSymlinkMode::Follow,
        };

        assert_eq!(
            transfer_import_header_pairs(&metadata),
            vec![
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    transfer_destination_path_header_value("/tmp/output"),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "zstd".to_string()),
                (TRANSFER_SYMLINK_MODE_HEADER, "follow".to_string()),
            ]
        );
    }

    #[test]
    fn transfer_header_pairs_render_canonical_export_metadata() {
        let metadata = TransferExportMetadata {
            source_type: TransferSourceType::Multiple,
            compression: TransferCompression::None,
        };

        assert_eq!(
            transfer_export_header_pairs(&metadata),
            vec![
                (TRANSFER_SOURCE_TYPE_HEADER, "multiple".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
            ]
        );
    }

    #[test]
    fn transfer_header_parser_reads_import_metadata_and_optional_defaults() {
        let headers = transfer_headers(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "L3RtcC9vdXRwdXQ="),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let parsed = parse_transfer_import_metadata(&headers).unwrap();

        assert_eq!(
            parsed,
            TransferImportMetadata {
                destination_path: "/tmp/output".to_string(),
                overwrite: TransferOverwrite::Merge,
                create_parent: false,
                source_type: TransferSourceType::File,
                compression: TransferCompression::None,
                symlink_mode: TransferSymlinkMode::Preserve,
            }
        );
    }

    #[test]
    fn transfer_header_parser_rejects_missing_required_import_headers() {
        for missing in [
            TRANSFER_DESTINATION_PATH_HEADER,
            TRANSFER_OVERWRITE_HEADER,
            TRANSFER_CREATE_PARENT_HEADER,
            TRANSFER_SOURCE_TYPE_HEADER,
        ] {
            let mut headers = transfer_headers(&[
                (TRANSFER_DESTINATION_PATH_HEADER, "L3RtcC9vdXRwdXQ="),
                (TRANSFER_OVERWRITE_HEADER, "merge"),
                (TRANSFER_CREATE_PARENT_HEADER, "true"),
                (TRANSFER_SOURCE_TYPE_HEADER, "file"),
            ]);
            match missing {
                TRANSFER_DESTINATION_PATH_HEADER => headers.destination_path = None,
                TRANSFER_OVERWRITE_HEADER => headers.overwrite = None,
                TRANSFER_CREATE_PARENT_HEADER => headers.create_parent = None,
                TRANSFER_SOURCE_TYPE_HEADER => headers.source_type = None,
                _ => unreachable!(),
            }

            let err = parse_transfer_import_metadata(&headers).unwrap_err();

            assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
            assert_eq!(err.header, missing);
        }
    }

    #[test]
    fn transfer_header_parser_rejects_invalid_import_metadata_values() {
        for (header, value) in [
            (TRANSFER_OVERWRITE_HEADER, "clobber"),
            (TRANSFER_CREATE_PARENT_HEADER, "yes"),
            (TRANSFER_SOURCE_TYPE_HEADER, "folder"),
            (TRANSFER_COMPRESSION_HEADER, "gzip"),
            (TRANSFER_SYMLINK_MODE_HEADER, "copy"),
        ] {
            let mut headers = transfer_headers(&[
                (TRANSFER_DESTINATION_PATH_HEADER, "L3RtcC9vdXRwdXQ="),
                (TRANSFER_OVERWRITE_HEADER, "merge"),
                (TRANSFER_CREATE_PARENT_HEADER, "true"),
                (TRANSFER_SOURCE_TYPE_HEADER, "file"),
                (TRANSFER_COMPRESSION_HEADER, "none"),
                (TRANSFER_SYMLINK_MODE_HEADER, "preserve"),
            ]);
            match header {
                TRANSFER_OVERWRITE_HEADER => headers.overwrite = Some(value.to_string()),
                TRANSFER_CREATE_PARENT_HEADER => headers.create_parent = Some(value.to_string()),
                TRANSFER_SOURCE_TYPE_HEADER => headers.source_type = Some(value.to_string()),
                TRANSFER_COMPRESSION_HEADER => headers.compression = Some(value.to_string()),
                TRANSFER_SYMLINK_MODE_HEADER => headers.symlink_mode = Some(value.to_string()),
                _ => unreachable!(),
            }

            let err = parse_transfer_import_metadata(&headers).unwrap_err();

            assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
            assert_eq!(err.header, header);
        }
    }

    #[test]
    fn transfer_header_parser_rejects_destination_path_with_newlines() {
        let headers = transfer_headers(&[
            (
                TRANSFER_DESTINATION_PATH_HEADER,
                "/tmp/output\r\nx-injected: nope",
            ),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let err = parse_transfer_import_metadata(&headers).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_DESTINATION_PATH_HEADER);
    }

    #[test]
    fn transfer_header_parser_round_trips_unicode_destination_path() {
        let headers = transfer_headers(&[
            (
                TRANSFER_DESTINATION_PATH_HEADER,
                "L3RtcC/mtYvor5Uv0L/RgNC40LLQtdGCL3LDqXN1bcOpLnR4dA==",
            ),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let parsed = parse_transfer_import_metadata(&headers).unwrap();

        assert_eq!(parsed.destination_path, "/tmp/测试/привет/résumé.txt");
    }

    #[test]
    fn transfer_header_parser_rejects_non_base64_destination_path() {
        let headers = transfer_headers(&[
            (TRANSFER_DESTINATION_PATH_HEADER, "/tmp/output"),
            (TRANSFER_OVERWRITE_HEADER, "merge"),
            (TRANSFER_CREATE_PARENT_HEADER, "false"),
            (TRANSFER_SOURCE_TYPE_HEADER, "file"),
        ]);

        let err = parse_transfer_import_metadata(&headers).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_DESTINATION_PATH_HEADER);
    }

    #[test]
    fn transfer_header_parser_reads_export_metadata_defaults() {
        let headers = transfer_headers(&[(TRANSFER_SOURCE_TYPE_HEADER, "directory")]);

        let parsed = parse_transfer_export_metadata(&headers).unwrap();

        assert_eq!(
            parsed,
            TransferExportMetadata {
                source_type: TransferSourceType::Directory,
                compression: TransferCompression::None,
            }
        );
    }

    #[test]
    fn transfer_header_parser_rejects_invalid_export_metadata() {
        let missing = TransferHeaders::default();
        let err = parse_transfer_export_metadata(&missing).unwrap_err();
        assert_eq!(err.kind, TransferHeaderErrorKind::Missing);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);

        let invalid = transfer_headers(&[(TRANSFER_SOURCE_TYPE_HEADER, "folder")]);
        let err = parse_transfer_export_metadata(&invalid).unwrap_err();
        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);
    }

    #[test]
    fn transfer_header_parser_rejects_header_values_with_newlines() {
        let headers = TransferHeaders {
            source_type: Some("directory\nremote".to_string()),
            ..TransferHeaders::default()
        };

        let err = parse_transfer_export_metadata(&headers).unwrap_err();

        assert_eq!(err.kind, TransferHeaderErrorKind::Invalid);
        assert_eq!(err.header, TRANSFER_SOURCE_TYPE_HEADER);
    }
}
