use std::path::{Component, Path, PathBuf};

use remote_exec_proto::public::{
    TransferEndpoint, TransferFilesInput, TransferFilesResult, TransferOverwrite,
    TransferSourceType as PublicTransferSourceType,
};
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse,
    TransferOverwriteMode, TransferSourceType as RpcTransferSourceType,
};

use crate::daemon_client::DaemonClientError;
use crate::mcp_server::ToolCallOutput;

pub async fn transfer_files(
    state: &crate::BrokerState,
    input: TransferFilesInput,
) -> anyhow::Result<ToolCallOutput> {
    ensure_absolute(&input.source)?;
    ensure_absolute(&input.destination)?;
    ensure_distinct_endpoints(&input.source, &input.destination)?;

    let temp = tempfile::NamedTempFile::new()?;
    let archive_path = temp.path().to_path_buf();
    let source_type = export_endpoint_to_archive(state, &input.source, &archive_path).await?;
    let summary = import_archive_to_endpoint(
        state,
        &archive_path,
        &input.destination,
        &input.overwrite,
        &source_type,
        input.create_parent,
    )
    .await?;

    let result = TransferFilesResult {
        source: input.source,
        destination: input.destination,
        source_type: match source_type {
            RpcTransferSourceType::File => PublicTransferSourceType::File,
            RpcTransferSourceType::Directory => PublicTransferSourceType::Directory,
        },
        bytes_copied: summary.bytes_copied,
        files_copied: summary.files_copied,
        directories_copied: summary.directories_copied,
        replaced: summary.replaced,
    };

    Ok(ToolCallOutput::text_and_structured(
        format_transfer_text(&result),
        serde_json::to_value(result)?,
    ))
}

async fn export_endpoint_to_archive(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
    archive_path: &Path,
) -> anyhow::Result<RpcTransferSourceType> {
    match endpoint.target.as_str() {
        "local" => crate::local_transfer::export_path_to_archive(Path::new(&endpoint.path), archive_path).await,
        target_name => {
            let target = state.target(target_name)?;
            target.ensure_identity_verified(target_name).await?;
            match target
                .client
                .transfer_export_to_file(
                    &TransferExportRequest {
                        path: endpoint.path.clone(),
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
    };

    match endpoint.target.as_str() {
        "local" => crate::local_transfer::import_archive_from_file(archive_path, &request).await,
        target_name => {
            let target = state.target(target_name)?;
            target.ensure_identity_verified(target_name).await?;
            match target
                .client
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

fn ensure_absolute(endpoint: &TransferEndpoint) -> anyhow::Result<()> {
    anyhow::ensure!(
        Path::new(&endpoint.path).is_absolute(),
        "transfer endpoint path `{}` is not absolute",
        endpoint.path
    );
    Ok(())
}

fn ensure_distinct_endpoints(source: &TransferEndpoint, destination: &TransferEndpoint) -> anyhow::Result<()> {
    anyhow::ensure!(
        !(source.target == destination.target
            && normalize_path(Path::new(&source.path)) == normalize_path(Path::new(&destination.path))),
        "source and destination must differ"
    );
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn normalize_transfer_error(err: DaemonClientError) -> anyhow::Error {
    match err {
        DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}

fn format_transfer_text(result: &TransferFilesResult) -> String {
    format!(
        "Transferred {} `{}` from `{}` to `{}` on `{}`.\nFiles: {}, directories: {}, bytes: {}, replaced: {}",
        match result.source_type {
            PublicTransferSourceType::File => "file",
            PublicTransferSourceType::Directory => "directory",
        },
        result.source.path,
        result.source.target,
        result.destination.path,
        result.destination.target,
        result.files_copied,
        result.directories_copied,
        result.bytes_copied,
        if result.replaced { "yes" } else { "no" }
    )
}
