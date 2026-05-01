use std::path::Path;

use remote_exec_proto::public::{
    TransferEndpoint, TransferOverwrite, TransferSymlinkMode as PublicTransferSymlinkMode,
};
use remote_exec_proto::rpc::{
    TransferCompression as RpcTransferCompression, TransferExportRequest, TransferImportRequest,
    TransferImportResponse, TransferOverwriteMode, TransferSourceType as RpcTransferSourceType,
    TransferSymlinkMode as RpcTransferSymlinkMode, TransferWarning,
};

use crate::daemon_client::DaemonClientError;

use super::endpoints::{endpoint_policy, verified_remote_target};

struct ExportedSourceArchive {
    endpoint: TransferEndpoint,
    source_policy: remote_exec_proto::path::PathPolicy,
    source_type: RpcTransferSourceType,
    warnings: Vec<TransferWarning>,
    temp_path: tempfile::TempPath,
}

struct ExportArchiveResult {
    source_type: RpcTransferSourceType,
    warnings: Vec<TransferWarning>,
}

pub(super) async fn transfer_single_source(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
    create_parent: bool,
) -> anyhow::Result<(RpcTransferSourceType, TransferImportResponse)> {
    let temp = tempfile::NamedTempFile::new()?;
    let archive_path = temp.into_temp_path();
    let exported = export_endpoint_to_archive(
        state,
        source,
        archive_path.as_ref(),
        compression,
        symlink_mode,
    )
    .await?;
    let request = build_import_request(
        destination,
        overwrite,
        exported.source_type.clone(),
        compression,
        symlink_mode,
        create_parent,
    );
    let mut summary =
        import_archive_to_endpoint(state, archive_path.as_ref(), destination, &request).await?;
    summary.warnings.splice(0..0, exported.warnings);
    Ok((exported.source_type, summary))
}

pub(super) async fn transfer_multiple_sources(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
    create_parent: bool,
) -> anyhow::Result<(RpcTransferSourceType, TransferImportResponse)> {
    let mut exported_sources = Vec::with_capacity(sources.len());
    for source in sources {
        let temp = tempfile::NamedTempFile::new()?;
        let temp_path = temp.into_temp_path();
        let source_policy = endpoint_policy(state, source).await?;
        let exported = export_endpoint_to_archive(
            state,
            source,
            temp_path.as_ref(),
            compression,
            symlink_mode,
        )
        .await?;
        exported_sources.push(ExportedSourceArchive {
            endpoint: source.clone(),
            source_policy,
            source_type: exported.source_type,
            warnings: exported.warnings,
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
                compression: compression.clone(),
                archive_path: source.temp_path.to_path_buf(),
            })
            .collect(),
        bundled_path.as_ref(),
        compression.clone(),
    )
    .await?;

    let source_type = RpcTransferSourceType::Multiple;
    let request = build_import_request(
        destination,
        overwrite,
        source_type.clone(),
        compression,
        symlink_mode,
        create_parent,
    );
    let mut summary =
        import_archive_to_endpoint(state, bundled_path.as_ref(), destination, &request).await?;
    let export_warnings = exported_sources
        .into_iter()
        .flat_map(|source| source.warnings)
        .collect::<Vec<_>>();
    summary.warnings.splice(0..0, export_warnings);
    Ok((source_type, summary))
}

async fn export_endpoint_to_archive(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
    archive_path: &Path,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
) -> anyhow::Result<ExportArchiveResult> {
    let request = TransferExportRequest {
        path: endpoint.path.clone(),
        compression: compression.clone(),
        symlink_mode: to_rpc_symlink_mode(symlink_mode),
    };

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
                warnings: exported.warnings,
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
        warnings: exported.warnings,
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

fn build_import_request(
    endpoint: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    source_type: RpcTransferSourceType,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
    create_parent: bool,
) -> TransferImportRequest {
    TransferImportRequest {
        destination_path: endpoint.path.clone(),
        overwrite: match overwrite {
            TransferOverwrite::Fail => TransferOverwriteMode::Fail,
            TransferOverwrite::Merge => TransferOverwriteMode::Merge,
            TransferOverwrite::Replace => TransferOverwriteMode::Replace,
        },
        create_parent,
        source_type,
        compression: compression.clone(),
        symlink_mode: to_rpc_symlink_mode(symlink_mode),
    }
}

fn to_rpc_symlink_mode(mode: &PublicTransferSymlinkMode) -> RpcTransferSymlinkMode {
    match mode {
        PublicTransferSymlinkMode::Preserve => RpcTransferSymlinkMode::Preserve,
        PublicTransferSymlinkMode::Follow => RpcTransferSymlinkMode::Follow,
        PublicTransferSymlinkMode::Skip => RpcTransferSymlinkMode::Skip,
        PublicTransferSymlinkMode::Reject => RpcTransferSymlinkMode::Reject,
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
