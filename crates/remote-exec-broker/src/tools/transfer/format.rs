use std::time::Instant;

use remote_exec_proto::public::{
    TransferEndpoint, TransferFilesResult, TransferSourceType as PublicTransferSourceType,
};
use remote_exec_proto::rpc::{
    TransferCompression as RpcTransferCompression, TransferImportResponse,
    TransferSourceType as RpcTransferSourceType,
};

use crate::mcp_server::ToolCallOutput;

pub(super) fn finish_transfer(
    started: Instant,
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

pub(super) fn format_transfer_compression(compression: &RpcTransferCompression) -> &'static str {
    match compression {
        RpcTransferCompression::None => "none",
        RpcTransferCompression::Zstd => "zstd",
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
