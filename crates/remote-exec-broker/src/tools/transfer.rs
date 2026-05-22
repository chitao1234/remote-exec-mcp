pub(crate) mod codec;
mod endpoints;
mod format;
mod operations;
mod plan;

use remote_exec_proto::public::TransferFilesInput;

use crate::mcp_server::ToolCallOutput;
use format::{CompletedTransfer, finish_transfer, format_transfer_compression};
use operations::execute_transfer_plan;
use plan::{TransferPlanRequest, plan_transfer};

pub async fn transfer_files(
    state: &crate::BrokerState,
    input: TransferFilesInput,
) -> anyhow::Result<ToolCallOutput> {
    let request = TransferPlanRequest::from_input(input)?;
    crate::request_context::set_current_targets(request.input_targets());
    let plan = plan_transfer(state, request).await?;

    tracing::info!(
        tool = "transfer_files",
        source_count = plan.sources.len(),
        first_source_target = %plan.first_source_target(),
        first_source_path = %plan.first_source_path(),
        destination_target = %plan.requested_destination.target,
        destination_path = %plan.requested_destination.path,
        compression = format_transfer_compression(&plan.compression),
        exclude_count = plan.exclude.len(),
        overwrite = ?plan.overwrite,
        destination_mode = ?plan.destination_mode,
        symlink_mode = ?plan.symlink_mode,
        create_parent = plan.create_parent,
        "transfer plan ready"
    );

    let (source_type, summary) = execute_transfer_plan(state, &plan).await?;

    finish_transfer(
        &plan.sources,
        CompletedTransfer {
            requested_destination: plan.requested_destination.clone(),
            destination: plan.destination.clone(),
            destination_mode: plan.destination_mode.clone(),
            symlink_mode: plan.symlink_mode.clone(),
            source_type,
            summary,
        },
    )
}
