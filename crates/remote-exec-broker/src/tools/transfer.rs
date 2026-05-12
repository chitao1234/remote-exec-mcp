pub(crate) mod codec;
mod endpoints;
mod format;
mod operations;

use remote_exec_proto::public::{TransferEndpoint, TransferFilesInput};

use crate::mcp_server::ToolCallOutput;
use endpoints::{
    ensure_absolute, ensure_distinct_endpoints, ensure_multi_source_basenames_are_unique,
    negotiate_transfer_compression, resolve_destination,
};
use format::{CompletedTransfer, finish_transfer, format_transfer_compression};
use operations::{TransferExecutionOptions, transfer_multiple_sources, transfer_single_source};

pub async fn transfer_files(
    state: &crate::BrokerState,
    input: TransferFilesInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = std::time::Instant::now();
    set_transfer_target_context(&input);
    let sources = resolve_sources(&input)?;
    let requested_destination = input.destination.clone();
    let compression =
        negotiate_transfer_compression(state, &sources, &requested_destination).await?;
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
        destination_target = %requested_destination.target,
        destination_path = %requested_destination.path,
        compression = format_transfer_compression(&compression),
        exclude_count = input.exclude.len(),
        overwrite = ?input.overwrite,
        destination_mode = ?input.destination_mode,
        symlink_mode = ?input.symlink_mode,
        create_parent = input.create_parent,
        "broker tool started"
    );

    for source in &sources {
        ensure_absolute(state, source).await?;
    }
    ensure_absolute(state, &requested_destination).await?;
    ensure_multi_source_basenames_are_unique(state, &sources, &requested_destination).await?;
    let destination = resolve_destination(
        state,
        &sources,
        &requested_destination,
        &input.destination_mode,
    )
    .await?;
    for source in &sources {
        ensure_distinct_endpoints(state, source, &destination).await?;
    }

    let (source_type, summary) = match sources.as_slice() {
        [source] => {
            let options = TransferExecutionOptions {
                overwrite: &input.overwrite,
                compression: &compression,
                exclude: &input.exclude,
                symlink_mode: &input.symlink_mode,
                create_parent: input.create_parent,
            };
            transfer_single_source(state, source, &destination, options).await?
        }
        _ => {
            let options = TransferExecutionOptions {
                overwrite: &input.overwrite,
                compression: &compression,
                exclude: &input.exclude,
                symlink_mode: &input.symlink_mode,
                create_parent: input.create_parent,
            };
            transfer_multiple_sources(state, &sources, &destination, options).await?
        }
    };

    finish_transfer(
        started,
        &sources,
        CompletedTransfer {
            requested_destination,
            destination,
            destination_mode: input.destination_mode,
            symlink_mode: input.symlink_mode,
            source_type,
            summary,
        },
    )
}

fn resolve_sources(input: &TransferFilesInput) -> anyhow::Result<Vec<TransferEndpoint>> {
    match (&input.source, input.sources.is_empty()) {
        (Some(_), false) => anyhow::bail!("provide either `source` or `sources`, not both"),
        (Some(source), true) => Ok(vec![source.clone()]),
        (None, false) => Ok(input.sources.clone()),
        (None, true) => anyhow::bail!("`sources` must contain at least one entry"),
    }
}

fn set_transfer_target_context(input: &TransferFilesInput) {
    let mut targets = Vec::new();
    if let Some(source) = &input.source {
        targets.push(source.target.as_str());
    }
    targets.extend(input.sources.iter().map(|source| source.target.as_str()));
    targets.push(input.destination.target.as_str());
    targets.retain(|target| !target.is_empty());
    targets.sort_unstable();
    targets.dedup();
    if !targets.is_empty() {
        crate::request_context::set_current_target(targets.join(","));
    }
}
