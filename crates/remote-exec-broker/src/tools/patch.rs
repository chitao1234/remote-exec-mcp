use remote_exec_proto::public::ApplyPatchInput;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse};

use crate::mcp_server::ToolCallOutput;

pub async fn forward_patch(
    state: &crate::BrokerState,
    target_name: &str,
    patch: String,
    workdir: Option<String>,
) -> anyhow::Result<String> {
    let response = state
        .patch_apply(target_name, &PatchApplyRequest { patch, workdir })
        .await?;
    log_patch_audit(target_name, &response);
    Ok(response.output)
}

fn log_patch_audit(target_name: &str, response: &PatchApplyResponse) {
    let preview = if response.updated_paths.is_empty() {
        String::new()
    } else {
        remote_exec_util::preview_text(&response.updated_paths.join(", "), 240)
    };
    let tool = crate::request_context::current()
        .map(|context| context.tool())
        .unwrap_or("apply_patch");

    tracing::info!(
        tool,
        target = %target_name,
        daemon_instance_id = response.daemon_instance_id.as_deref().unwrap_or("-"),
        updated_path_count = response.updated_paths.len(),
        updated_path_preview = %preview,
        "patch apply audit"
    );
}

pub async fn apply_patch(
    state: &crate::BrokerState,
    input: ApplyPatchInput,
) -> anyhow::Result<ToolCallOutput> {
    crate::request_context::set_current_target(input.target.clone());
    let patch_len = input.input.len();
    tracing::info!(
        tool = "apply_patch",
        target = %input.target,
        patch_len,
        has_workdir = input.workdir.is_some(),
        "patch apply requested"
    );
    let output = forward_patch(state, &input.target, input.input, input.workdir).await?;

    Ok(ToolCallOutput::text(output))
}
