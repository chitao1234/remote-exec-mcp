mod endpoints;
mod format;
mod operations;

use remote_exec_proto::public::{TransferEndpoint, TransferFilesInput};

use crate::mcp_server::ToolCallOutput;
use endpoints::{
    ensure_absolute, ensure_distinct_endpoints, ensure_multi_source_basenames_are_unique,
    negotiate_transfer_compression,
};
use format::{finish_transfer, format_transfer_compression};
use operations::{transfer_multiple_sources, transfer_single_source};

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

fn resolve_sources(input: &TransferFilesInput) -> anyhow::Result<Vec<TransferEndpoint>> {
    match (&input.source, input.sources.is_empty()) {
        (Some(_), false) => anyhow::bail!("provide either `source` or `sources`, not both"),
        (Some(source), true) => Ok(vec![source.clone()]),
        (None, false) => Ok(input.sources.clone()),
        (None, true) => anyhow::bail!("`sources` must contain at least one entry"),
    }
}
