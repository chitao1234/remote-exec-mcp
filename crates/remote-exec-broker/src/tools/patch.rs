use remote_exec_proto::public::{ApplyPatchInput, ApplyPatchResult};
use remote_exec_proto::rpc::PatchApplyRequest;

use crate::mcp_server::ToolCallOutput;

pub async fn apply_patch(
    state: &crate::BrokerState,
    input: ApplyPatchInput,
) -> anyhow::Result<ToolCallOutput> {
    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
    let response = target
        .client
        .patch_apply(&PatchApplyRequest {
            patch: input.input,
            workdir: input.workdir,
        })
        .await?;

    Ok(ToolCallOutput::text_and_structured(
        response.output.clone(),
        serde_json::to_value(ApplyPatchResult {
            target: input.target,
            output: response.output,
        })?,
    ))
}
