use std::time::Instant;

use remote_exec_proto::public::{
    TransferDestinationMode, TransferEndpoint, TransferFilesResult,
    TransferSourceType as PublicTransferSourceType,
    TransferSymlinkMode as PublicTransferSymlinkMode,
};
use remote_exec_proto::rpc::{
    TransferCompression as RpcTransferCompression, TransferImportResponse,
    TransferSourceType as RpcTransferSourceType,
};

use crate::mcp_server::ToolCallOutput;

pub(super) struct CompletedTransfer {
    pub requested_destination: TransferEndpoint,
    pub destination: TransferEndpoint,
    pub destination_mode: TransferDestinationMode,
    pub symlink_mode: PublicTransferSymlinkMode,
    pub source_type: RpcTransferSourceType,
    pub summary: TransferImportResponse,
}

pub(super) fn finish_transfer(
    started: Instant,
    sources: &[TransferEndpoint],
    completed: CompletedTransfer,
) -> anyhow::Result<ToolCallOutput> {
    let destination_target = completed.destination.target.clone();
    let destination_path = completed.destination.path.clone();
    let result = TransferFilesResult {
        source: (sources.len() == 1).then(|| sources[0].clone()),
        sources: sources.to_vec(),
        destination: completed.requested_destination,
        resolved_destination: completed.destination,
        destination_mode: completed.destination_mode,
        symlink_mode: completed.symlink_mode,
        source_type: match completed.source_type {
            RpcTransferSourceType::File => PublicTransferSourceType::File,
            RpcTransferSourceType::Directory => PublicTransferSourceType::Directory,
            RpcTransferSourceType::Multiple => PublicTransferSourceType::Multiple,
        },
        bytes_copied: completed.summary.bytes_copied,
        files_copied: completed.summary.files_copied,
        directories_copied: completed.summary.directories_copied,
        replaced: completed.summary.replaced,
        warnings: completed.summary.warnings,
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

    let summary = format!(
        "Transferred {} to `{}` on `{}`.\nFiles: {}, directories: {}, bytes: {}, replaced: {}",
        source_summary,
        result.resolved_destination.path,
        result.resolved_destination.target,
        result.files_copied,
        result.directories_copied,
        result.bytes_copied,
        if result.replaced { "yes" } else { "no" }
    );

    if result.warnings.is_empty() {
        return summary;
    }

    let warning_text = if result.warnings.len() == 1 {
        format!("Warning: {}", result.warnings[0].message)
    } else {
        format!(
            "Warnings:\n{}",
            result
                .warnings
                .iter()
                .map(|warning| format!("- {}", warning.message))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!("{warning_text}\n\n{summary}")
}
