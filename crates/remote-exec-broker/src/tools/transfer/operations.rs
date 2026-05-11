use std::path::Path;

use remote_exec_proto::public::{TransferEndpoint, TransferOverwrite, TransferSymlinkMode};
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse, TransferSourceType,
};
use remote_exec_proto::transfer::TransferCompression;

use crate::daemon_client::DaemonClientError;

use super::endpoints::{endpoint_policy, verified_remote_target};

struct ExportedSourceArchive {
    endpoint: TransferEndpoint,
    source_policy: remote_exec_proto::path::PathPolicy,
    source_type: TransferSourceType,
    temp_path: tempfile::TempPath,
}

struct ExportArchiveResult {
    source_type: TransferSourceType,
}

#[derive(Clone, Copy)]
pub(super) struct TransferExecutionOptions<'a> {
    pub(super) overwrite: &'a TransferOverwrite,
    pub(super) compression: &'a TransferCompression,
    pub(super) exclude: &'a [String],
    pub(super) symlink_mode: &'a TransferSymlinkMode,
    pub(super) create_parent: bool,
}

pub(super) async fn transfer_single_source(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
    options: TransferExecutionOptions<'_>,
) -> anyhow::Result<(TransferSourceType, TransferImportResponse)> {
    match (source.target.as_str(), destination.target.as_str()) {
        ("local", "local") => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let exported = crate::local_transfer::export_path_to_stream(
                &source.path,
                &export_request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            let request = build_import_request(
                destination,
                options.overwrite,
                exported.source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let summary = crate::local_transfer::import_archive_from_async_reader(
                exported.reader,
                &request,
                state.host_sandbox.as_ref(),
                state.transfer_limits,
            )
            .await?;
            Ok((exported.source_type, summary))
        }
        ("local", target_name) => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let exported = crate::local_transfer::export_path_to_stream(
                &source.path,
                &export_request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            let request = build_import_request(
                destination,
                options.overwrite,
                exported.source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let body =
                reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(exported.reader));
            let summary =
                import_remote_body_to_endpoint(state, target_name, body, &request).await?;
            Ok((exported.source_type, summary))
        }
        (target_name, "local") => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let target = verified_remote_target(state, target_name).await?;
            let exported = handle_remote_transfer_result(
                target,
                target.transfer_export_stream(&export_request).await,
            )
            .await?;
            let source_type = exported.source_type.clone();
            let request = build_import_request(
                destination,
                options.overwrite,
                source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let summary = crate::local_transfer::import_archive_from_async_reader(
                exported.into_async_read(),
                &request,
                state.host_sandbox.as_ref(),
                state.transfer_limits,
            )
            .await?;
            Ok((source_type, summary))
        }
        (source_target_name, destination_target_name) => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let source_target = verified_remote_target(state, source_target_name).await?;
            let exported = handle_remote_transfer_result(
                source_target,
                source_target.transfer_export_stream(&export_request).await,
            )
            .await?;
            let source_type = exported.source_type.clone();
            let request = build_import_request(
                destination,
                options.overwrite,
                source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let summary = import_remote_body_to_endpoint(
                state,
                destination_target_name,
                exported.into_body(),
                &request,
            )
            .await?;
            Ok((source_type, summary))
        }
    }
}

pub(super) async fn transfer_multiple_sources(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
    options: TransferExecutionOptions<'_>,
) -> anyhow::Result<(TransferSourceType, TransferImportResponse)> {
    let mut exported_sources = Vec::with_capacity(sources.len());
    for source in sources {
        let temp = tempfile::NamedTempFile::new()?;
        let temp_path = temp.into_temp_path();
        let source_policy = endpoint_policy(state, source).await?;
        let exported = export_endpoint_to_archive(
            state,
            source,
            temp_path.as_ref(),
            options.compression,
            options.exclude,
            options.symlink_mode,
        )
        .await?;
        exported_sources.push(ExportedSourceArchive {
            endpoint: source.clone(),
            source_policy,
            source_type: exported.source_type,
            temp_path,
        });
    }

    let bundled = tempfile::NamedTempFile::new()?;
    let bundled_path = bundled.into_temp_path();
    crate::local_transfer::bundle_archives_to_file(
        exported_sources
            .iter()
            .map(|source| crate::local_transfer::BundledArchiveSource {
                source_path: source.endpoint.path.clone(),
                source_policy: source.source_policy,
                source_type: source.source_type.clone(),
                compression: options.compression.clone(),
                archive_path: source.temp_path.to_path_buf(),
            })
            .collect(),
        bundled_path.as_ref(),
        options.compression.clone(),
    )
    .await?;

    let source_type = TransferSourceType::Multiple;
    let request = build_import_request(
        destination,
        options.overwrite,
        source_type.clone(),
        options.compression,
        options.symlink_mode,
        options.create_parent,
    );
    let summary =
        import_archive_to_endpoint(state, bundled_path.as_ref(), destination, &request).await?;
    Ok((source_type, summary))
}

fn build_export_request(
    endpoint: &TransferEndpoint,
    compression: &TransferCompression,
    exclude: &[String],
    symlink_mode: &TransferSymlinkMode,
) -> TransferExportRequest {
    TransferExportRequest {
        path: endpoint.path.clone(),
        compression: compression.clone(),
        symlink_mode: symlink_mode.clone(),
        exclude: exclude.to_vec(),
    }
}

async fn export_endpoint_to_archive(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
    archive_path: &Path,
    compression: &TransferCompression,
    exclude: &[String],
    symlink_mode: &TransferSymlinkMode,
) -> anyhow::Result<ExportArchiveResult> {
    let request = build_export_request(endpoint, compression, exclude, symlink_mode);

    match endpoint.target.as_str() {
        "local" => {
            let exported = crate::local_transfer::export_path_to_archive(
                &endpoint.path,
                archive_path,
                &request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            Ok(ExportArchiveResult {
                source_type: exported.source_type,
            })
        }
        target_name => {
            export_remote_endpoint_to_archive(state, target_name, &request, archive_path).await
        }
    }
}

async fn import_archive_to_endpoint(
    state: &crate::BrokerState,
    archive_path: &Path,
    endpoint: &TransferEndpoint,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    match endpoint.target.as_str() {
        "local" => {
            crate::local_transfer::import_archive_from_file(
                archive_path,
                request,
                state.host_sandbox.as_ref(),
                state.transfer_limits,
            )
            .await
        }
        target_name => {
            import_remote_archive_to_endpoint(state, target_name, archive_path, request).await
        }
    }
}

fn normalize_transfer_error(err: DaemonClientError) -> anyhow::Error {
    match err {
        DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}

async fn export_remote_endpoint_to_archive(
    state: &crate::BrokerState,
    target_name: &str,
    request: &TransferExportRequest,
    archive_path: &Path,
) -> anyhow::Result<ExportArchiveResult> {
    let target = verified_remote_target(state, target_name).await?;
    let exported = handle_remote_transfer_result(
        target,
        target.transfer_export_to_file(request, archive_path).await,
    )
    .await?;
    Ok(ExportArchiveResult {
        source_type: exported.source_type,
    })
}

async fn import_remote_archive_to_endpoint(
    state: &crate::BrokerState,
    target_name: &str,
    archive_path: &Path,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    let target = verified_remote_target(state, target_name).await?;
    handle_remote_transfer_result(
        target,
        target
            .transfer_import_from_file(archive_path, request)
            .await,
    )
    .await
}

async fn import_remote_body_to_endpoint(
    state: &crate::BrokerState,
    target_name: &str,
    body: reqwest::Body,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    let target = verified_remote_target(state, target_name).await?;
    handle_remote_transfer_result(
        target,
        target.transfer_import_from_body(request, body).await,
    )
    .await
}

fn build_import_request(
    endpoint: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    source_type: TransferSourceType,
    compression: &TransferCompression,
    symlink_mode: &TransferSymlinkMode,
    create_parent: bool,
) -> TransferImportRequest {
    TransferImportRequest {
        destination_path: endpoint.path.clone(),
        overwrite: overwrite.clone(),
        create_parent,
        source_type,
        compression: compression.clone(),
        symlink_mode: symlink_mode.clone(),
    }
}

async fn handle_remote_transfer_result<T>(
    target: &crate::TargetHandle,
    result: Result<T, DaemonClientError>,
) -> anyhow::Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            if matches!(err, DaemonClientError::Transport(_)) {
                target.clear_cached_daemon_info().await;
            }
            Err(normalize_transfer_error(err))
        }
    }
}
