use std::path::Path;

use anyhow::Context;
use remote_exec_proto::path::{
    PathPolicy, is_absolute_for_policy, linux_path_policy, same_path_for_policy,
    windows_path_policy,
};
use remote_exec_proto::public::{
    TransferEndpoint, TransferFilesInput, TransferFilesResult, TransferOverwrite,
    TransferSourceType as PublicTransferSourceType,
};
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse, TransferOverwriteMode,
    TransferSourceType as RpcTransferSourceType,
};

use crate::daemon_client::DaemonClientError;
use crate::mcp_server::ToolCallOutput;

pub async fn transfer_files(
    state: &crate::BrokerState,
    input: TransferFilesInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = std::time::Instant::now();
    let source_target = input.source.target.clone();
    let source_path = input.source.path.clone();
    let destination_target = input.destination.target.clone();
    let destination_path = input.destination.path.clone();
    tracing::info!(
        tool = "transfer_files",
        source_target = %source_target,
        source_path = %source_path,
        destination_target = %destination_target,
        destination_path = %destination_path,
        overwrite = ?input.overwrite,
        create_parent = input.create_parent,
        "broker tool started"
    );
    ensure_absolute(state, &input.source).await?;
    ensure_absolute(state, &input.destination).await?;
    ensure_distinct_endpoints(state, &input.source, &input.destination).await?;

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

    tracing::info!(
        tool = "transfer_files",
        source_target = %source_target,
        source_path = %source_path,
        destination_target = %destination_target,
        destination_path = %destination_path,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );
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
        "local" => {
            crate::local_transfer::export_path_to_archive(
                &endpoint.path,
                archive_path,
                state.host_sandbox.as_ref(),
            )
            .await
        }
        target_name => {
            let target = state.target(target_name)?;
            target.ensure_identity_verified(target_name).await?;
            match target
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
        "local" => {
            crate::local_transfer::import_archive_from_file(
                archive_path,
                &request,
                state.host_sandbox.as_ref(),
            )
            .await
        }
        target_name => {
            let target = state.target(target_name)?;
            target.ensure_identity_verified(target_name).await?;
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

fn local_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn remote_policy(platform: &str) -> PathPolicy {
    if platform.eq_ignore_ascii_case("windows") {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

async fn endpoint_policy(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<PathPolicy> {
    if endpoint.target == "local" {
        return Ok(local_policy());
    }

    let target = state.target(&endpoint.target)?;
    target.ensure_identity_verified(&endpoint.target).await?;
    let info = target
        .cached_daemon_info()
        .await
        .context("target info missing after identity verification")?;
    Ok(remote_policy(&info.platform))
}

async fn ensure_absolute(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<()> {
    let policy = endpoint_policy(state, endpoint).await?;
    anyhow::ensure!(
        is_absolute_for_policy(policy, &endpoint.path),
        "transfer endpoint path `{}` is not absolute",
        endpoint.path
    );
    Ok(())
}

async fn ensure_distinct_endpoints(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if source.target != destination.target {
        return Ok(());
    }

    let policy = endpoint_policy(state, source).await?;
    anyhow::ensure!(
        !same_path_for_policy(policy, &source.path, &destination.path),
        "source and destination must differ"
    );
    Ok(())
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
