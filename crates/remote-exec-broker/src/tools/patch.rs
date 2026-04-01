use remote_exec_proto::public::ApplyPatchInput;
use remote_exec_proto::rpc::PatchApplyRequest;

use crate::mcp_server::ToolCallOutput;

pub async fn forward_patch(
    state: &crate::BrokerState,
    target_name: &str,
    patch: String,
    workdir: Option<String>,
) -> anyhow::Result<String> {
    let target = state.target(target_name)?;
    target.ensure_identity_verified(target_name).await?;
    Ok(target
        .client
        .patch_apply(&PatchApplyRequest { patch, workdir })
        .await?
        .output)
}

pub async fn apply_patch(
    state: &crate::BrokerState,
    input: ApplyPatchInput,
) -> anyhow::Result<ToolCallOutput> {
    let output = forward_patch(state, &input.target, input.input, input.workdir).await?;

    Ok(ToolCallOutput::text_and_structured(output, serde_json::json!({})))
}
