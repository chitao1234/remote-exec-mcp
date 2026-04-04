use remote_exec_proto::public::ApplyPatchInput;
use remote_exec_proto::rpc::PatchApplyRequest;

use crate::daemon_client::DaemonClientError;
use crate::mcp_server::ToolCallOutput;

pub async fn forward_patch(
    state: &crate::BrokerState,
    target_name: &str,
    patch: String,
    workdir: Option<String>,
) -> anyhow::Result<String> {
    let target = state.target(target_name)?;
    target.ensure_identity_verified(target_name).await?;
    let response = match target
        .client
        .patch_apply(&PatchApplyRequest { patch, workdir })
        .await
    {
        Ok(response) => response,
        Err(err) => {
            if matches!(err, DaemonClientError::Transport(_)) {
                target.clear_cached_daemon_info().await;
            }
            return Err(err.into());
        }
    };
    Ok(response.output)
}

pub async fn apply_patch(
    state: &crate::BrokerState,
    input: ApplyPatchInput,
) -> anyhow::Result<ToolCallOutput> {
    let output = forward_patch(state, &input.target, input.input, input.workdir).await?;

    Ok(ToolCallOutput::text_and_structured(
        output,
        serde_json::json!({}),
    ))
}
