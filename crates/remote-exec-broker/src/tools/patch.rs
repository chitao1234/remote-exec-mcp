use remote_exec_proto::public::ApplyPatchInput;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse};

use crate::mcp_server::ToolCallOutput;

pub async fn forward_patch(
    state: &crate::BrokerState,
    target_name: &str,
    patch: String,
    workdir: Option<String>,
) -> anyhow::Result<String> {
    let target = state.target(target_name)?;
    let response = target
        .patch_apply_checked(target_name, &PatchApplyRequest { patch, workdir })
        .await?;
    log_patch_audit(target_name, &response);
    Ok(response.output)
}

fn log_patch_audit(target_name: &str, response: &PatchApplyResponse) {
    let preview = if response.updated_paths.is_empty() {
        String::new()
    } else {
        crate::logging::preview_text(&response.updated_paths.join(", "), 240)
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
    let started = std::time::Instant::now();
    let target_name = input.target.clone();
    crate::request_context::set_current_target(target_name.clone());
    let patch_len = input.input.len();
    tracing::info!(
        tool = "apply_patch",
        target = %target_name,
        patch_len,
        has_workdir = input.workdir.is_some(),
        "broker tool started"
    );
    let output = forward_patch(state, &input.target, input.input, input.workdir)
        .await
        .inspect_err(|err| {
            tracing::warn!(
                tool = "apply_patch",
                target = %target_name,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "broker tool failed"
            );
        })?;

    tracing::info!(
        tool = "apply_patch",
        target = %target_name,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );

    Ok(ToolCallOutput::text(output))
}
