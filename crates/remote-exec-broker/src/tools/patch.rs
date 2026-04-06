use remote_exec_proto::public::{ApplyPatchInput, ApplyPatchResult};
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
    let started = std::time::Instant::now();
    let target_name = input.target.clone();
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

    Ok(ToolCallOutput::text_and_structured(
        output.clone(),
        serde_json::to_value(ApplyPatchResult {
            success: true,
            output,
        })?,
    ))
}
