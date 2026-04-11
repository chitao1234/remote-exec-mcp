use std::path::Path;

use remote_exec_proto::public::{TransferEndpoint, TransferOverwrite};
use remote_exec_proto::rpc::{
    TransferCompression as RpcTransferCompression, TransferExportRequest, TransferImportRequest,
    TransferImportResponse, TransferOverwriteMode, TransferSourceType as RpcTransferSourceType,
};

use crate::daemon_client::DaemonClientError;

use super::endpoints::{endpoint_policy, verified_remote_target};

struct ExportedSourceArchive {
    endpoint: TransferEndpoint,
    source_policy: remote_exec_proto::path::PathPolicy,
    source_type: RpcTransferSourceType,
    temp_path: tempfile::TempPath,
}

pub(super) async fn transfer_single_source(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    compression: &RpcTransferCompression,
    create_parent: bool,
) -> anyhow::Result<(RpcTransferSourceType, TransferImportResponse)> {
    let temp = tempfile::NamedTempFile::new()?;
    let archive_path = temp.into_temp_path();
    let source_type =
        export_endpoint_to_archive(state, source, archive_path.as_ref(), compression).await?;
    let summary = import_archive_to_endpoint(
        state,
        archive_path.as_ref(),
        destination,
        overwrite,
        &source_type,
        compression,
        create_parent,
    )
    .await?;
    Ok((source_type, summary))
}

pub(super) async fn transfer_multiple_sources(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    compression: &RpcTransferCompression,
    create_parent: bool,
) -> anyhow::Result<(RpcTransferSourceType, TransferImportResponse)> {
    let mut exported = Vec::with_capacity(sources.len());
    for source in sources {
        let temp = tempfile::NamedTempFile::new()?;
        let temp_path = temp.into_temp_path();
        let source_policy = endpoint_policy(state, source).await?;
        let source_type =
            export_endpoint_to_archive(state, source, temp_path.as_ref(), compression).await?;
        exported.push(ExportedSourceArchive {
            endpoint: source.clone(),
            source_policy,
            source_type,
            temp_path,
        });
    }

    let bundled = tempfile::NamedTempFile::new()?;
    let bundled_path = bundled.into_temp_path();
    crate::local_transfer::bundle_archives_to_file(
        exported
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
    let summary = import_archive_to_endpoint(
        state,
        bundled_path.as_ref(),
        destination,
        overwrite,
        &source_type,
        compression,
        create_parent,
    )
    .await?;
    Ok((source_type, summary))
}

async fn export_endpoint_to_archive(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
    archive_path: &Path,
    compression: &RpcTransferCompression,
) -> anyhow::Result<RpcTransferSourceType> {
    match endpoint.target.as_str() {
        "local" => {
            crate::local_transfer::export_path_to_archive(
                &endpoint.path,
                archive_path,
                compression.clone(),
                state.host_sandbox.as_ref(),
            )
            .await
        }
        target_name => {
            let target = verified_remote_target(state, target_name).await?;
            match target
                .transfer_export_to_file(
                    &TransferExportRequest {
                        path: endpoint.path.clone(),
                        compression: compression.clone(),
                    },
                    archive_path,
                )
                .await
            {
                Ok(source_type) => Ok(source_type),
                Err(err) => {
                    if matches!(err, DaemonClientError::Transport(_)) {
                        target.clear_cached_daemon_info().await;
                    }
                    Err(normalize_transfer_error(err))
                }
            }
        }
    }
}

async fn import_archive_to_endpoint(
    state: &crate::BrokerState,
    archive_path: &Path,
    endpoint: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    source_type: &RpcTransferSourceType,
    compression: &RpcTransferCompression,
    create_parent: bool,
) -> anyhow::Result<TransferImportResponse> {
    let request = TransferImportRequest {
        destination_path: endpoint.path.clone(),
        overwrite: match overwrite {
            TransferOverwrite::Fail => TransferOverwriteMode::Fail,
            TransferOverwrite::Replace => TransferOverwriteMode::Replace,
        },
        create_parent,
        source_type: source_type.clone(),
        compression: compression.clone(),
    };

    match endpoint.target.as_str() {
        "local" => {
            crate::local_transfer::import_archive_from_file(
                archive_path,
                &request,
                state.host_sandbox.as_ref(),
            )
            .await
        }
        target_name => {
            let target = verified_remote_target(state, target_name).await?;
            match target
                .transfer_import_from_file(archive_path, &request)
                .await
            {
                Ok(summary) => Ok(summary),
                Err(err) => {
                    if matches!(err, DaemonClientError::Transport(_)) {
                        target.clear_cached_daemon_info().await;
                    }
                    Err(normalize_transfer_error(err))
                }
            }
        }
    }
}

fn normalize_transfer_error(err: DaemonClientError) -> anyhow::Error {
    match err {
        DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}
