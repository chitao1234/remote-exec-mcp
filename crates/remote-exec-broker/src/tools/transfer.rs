use std::path::Path;

use anyhow::Context;
use remote_exec_proto::path::{
    PathPolicy, basename_for_policy, is_absolute_for_policy, join_for_policy, linux_path_policy,
    same_path_for_policy, windows_path_policy,
};
use remote_exec_proto::public::{
    TransferEndpoint, TransferFilesInput, TransferFilesResult, TransferOverwrite,
    TransferSourceType as PublicTransferSourceType,
};
use remote_exec_proto::rpc::{
    TransferCompression as RpcTransferCompression, TransferExportRequest, TransferImportRequest,
    TransferImportResponse, TransferOverwriteMode, TransferSourceType as RpcTransferSourceType,
};

use crate::daemon_client::DaemonClientError;
use crate::mcp_server::ToolCallOutput;

struct ExportedSourceArchive {
    endpoint: TransferEndpoint,
    source_policy: PathPolicy,
    source_type: RpcTransferSourceType,
    temp_path: tempfile::TempPath,
}

pub async fn transfer_files(
    state: &crate::BrokerState,
    input: TransferFilesInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = std::time::Instant::now();
    let sources = resolve_sources(&input)?;
    let destination = input.destination.clone();
    let compression = negotiate_transfer_compression(state, &sources, &destination).await?;
    let first_source_target = sources
        .first()
        .map(|source| source.target.as_str())
        .unwrap_or("unknown");
    let first_source_path = sources
        .first()
        .map(|source| source.path.as_str())
        .unwrap_or("unknown");

    tracing::info!(
        tool = "transfer_files",
        source_count = sources.len(),
        first_source_target = %first_source_target,
        first_source_path = %first_source_path,
        destination_target = %destination.target,
        destination_path = %destination.path,
        compression = format_transfer_compression(&compression),
        overwrite = ?input.overwrite,
        create_parent = input.create_parent,
        "broker tool started"
    );

    for source in &sources {
        ensure_absolute(state, source).await?;
        ensure_distinct_endpoints(state, source, &destination).await?;
    }
    ensure_absolute(state, &destination).await?;
    ensure_multi_source_basenames_are_unique(state, &sources, &destination).await?;

    let (source_type, summary) = match sources.as_slice() {
        [source] => {
            transfer_single_source(
                state,
                source,
                &destination,
                &input.overwrite,
                &compression,
                input.create_parent,
            )
            .await?
        }
        _ => {
            transfer_multiple_sources(
                state,
                &sources,
                &destination,
                &input.overwrite,
                &compression,
                input.create_parent,
            )
            .await?
        }
    };

    finish_transfer(started, &sources, destination, source_type, summary)
}

fn finish_transfer(
    started: std::time::Instant,
    sources: &[TransferEndpoint],
    destination: TransferEndpoint,
    source_type: RpcTransferSourceType,
    summary: TransferImportResponse,
) -> anyhow::Result<ToolCallOutput> {
    let destination_target = destination.target.clone();
    let destination_path = destination.path.clone();
    let result = TransferFilesResult {
        source: (sources.len() == 1).then(|| sources[0].clone()),
        sources: sources.to_vec(),
        destination,
        source_type: match source_type {
            RpcTransferSourceType::File => PublicTransferSourceType::File,
            RpcTransferSourceType::Directory => PublicTransferSourceType::Directory,
            RpcTransferSourceType::Multiple => PublicTransferSourceType::Multiple,
        },
        bytes_copied: summary.bytes_copied,
        files_copied: summary.files_copied,
        directories_copied: summary.directories_copied,
        replaced: summary.replaced,
    };

    tracing::info!(
        tool = "transfer_files",
        source_count = sources.len(),
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

fn resolve_sources(input: &TransferFilesInput) -> anyhow::Result<Vec<TransferEndpoint>> {
    match (&input.source, input.sources.is_empty()) {
        (Some(_), false) => anyhow::bail!("provide either `source` or `sources`, not both"),
        (Some(source), true) => Ok(vec![source.clone()]),
        (None, false) => Ok(input.sources.clone()),
        (None, true) => anyhow::bail!("`sources` must contain at least one entry"),
    }
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

async fn transfer_single_source(
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

async fn transfer_multiple_sources(
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
    remote_exec_daemon::transfer::archive::bundle_archives_to_file(
        exported
            .iter()
            .map(
                |source| remote_exec_daemon::transfer::archive::BundledArchiveSource {
                    source_path: source.endpoint.path.clone(),
                    source_policy: source.source_policy,
                    source_type: source.source_type.clone(),
                    compression: compression.clone(),
                    archive_path: source.temp_path.to_path_buf(),
                },
            )
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

async fn verified_remote_target<'a>(
    state: &'a crate::BrokerState,
    target_name: &'a str,
) -> anyhow::Result<&'a crate::TargetHandle> {
    let target = state.target(target_name)?;
    target.ensure_identity_verified(target_name).await?;
    Ok(target)
}

async fn verified_remote_daemon_info(
    state: &crate::BrokerState,
    target_name: &str,
) -> anyhow::Result<crate::CachedDaemonInfo> {
    verified_remote_target(state, target_name)
        .await?
        .cached_daemon_info()
        .await
        .context("target info missing after identity verification")
}

async fn endpoint_policy(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<PathPolicy> {
    if endpoint.target == "local" {
        return Ok(local_policy());
    }

    let info = verified_remote_daemon_info(state, &endpoint.target).await?;
    Ok(remote_policy(&info.platform))
}

async fn ensure_absolute(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<()> {
    if endpoint.target == "local" {
        let policy = local_policy();
        anyhow::ensure!(
            is_absolute_for_policy(policy, &endpoint.path),
            "transfer endpoint path `{}` is not absolute",
            endpoint.path
        );
        return Ok(());
    }

    let info = verified_remote_daemon_info(state, &endpoint.target).await?;
    let policy = remote_policy(&info.platform);
    anyhow::ensure!(
        is_absolute_for_policy(policy, &endpoint.path)
            || (info.platform.eq_ignore_ascii_case("windows")
                && endpoint.path.starts_with('/')
                && !endpoint.path.starts_with("//")),
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

async fn ensure_multi_source_basenames_are_unique(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if sources.len() <= 1 {
        return Ok(());
    }

    let destination_policy = endpoint_policy(state, destination).await?;
    let mut seen_paths: Vec<String> = Vec::with_capacity(sources.len());
    for source in sources {
        let source_policy = endpoint_policy(state, source).await?;
        let basename = basename_for_policy(source_policy, &source.path).ok_or_else(|| {
            anyhow::anyhow!(
                "transfer source path `{}` has no usable basename for multi-source transfer",
                source.path
            )
        })?;
        let candidate = join_for_policy(destination_policy, &destination.path, &basename);
        anyhow::ensure!(
            !seen_paths.iter().any(|existing| same_path_for_policy(
                destination_policy,
                existing,
                &candidate
            )),
            "multi-source transfer would create duplicate destination entry `{basename}`"
        );
        seen_paths.push(candidate);
    }

    Ok(())
}

async fn negotiate_transfer_compression(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<RpcTransferCompression> {
    if !state.enable_transfer_compression {
        return Ok(RpcTransferCompression::None);
    }

    let mut has_remote_endpoint = false;
    for endpoint in sources.iter().chain(std::iter::once(destination)) {
        if endpoint.target == "local" {
            continue;
        }

        has_remote_endpoint = true;
        let info = verified_remote_daemon_info(state, &endpoint.target).await?;
        if !info.supports_transfer_compression {
            return Ok(RpcTransferCompression::None);
        }
    }

    if has_remote_endpoint {
        Ok(RpcTransferCompression::Zstd)
    } else {
        Ok(RpcTransferCompression::None)
    }
}

fn normalize_transfer_error(err: DaemonClientError) -> anyhow::Error {
    match err {
        DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}

fn format_transfer_text(result: &TransferFilesResult) -> String {
    let source_summary = match (&result.source, &result.source_type) {
        (Some(source), PublicTransferSourceType::File) => {
            format!("file `{}` from `{}`", source.path, source.target)
        }
        (Some(source), PublicTransferSourceType::Directory) => {
            format!("directory `{}` from `{}`", source.path, source.target)
        }
        _ => format!("{} sources", result.sources.len()),
    };

    format!(
        "Transferred {} to `{}` on `{}`.\nFiles: {}, directories: {}, bytes: {}, replaced: {}",
        source_summary,
        result.destination.path,
        result.destination.target,
        result.files_copied,
        result.directories_copied,
        result.bytes_copied,
        if result.replaced { "yes" } else { "no" }
    )
}

fn format_transfer_compression(compression: &RpcTransferCompression) -> &'static str {
    match compression {
        RpcTransferCompression::None => "none",
        RpcTransferCompression::Zstd => "zstd",
    }
}
