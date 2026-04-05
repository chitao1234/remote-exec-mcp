use anyhow::Context;
use remote_exec_proto::public::{ViewImageInput, ViewImageResult};
use remote_exec_proto::rpc::ImageReadRequest;
use rmcp::model::Content;

use crate::daemon_client::DaemonClientError;
use crate::mcp_server::ToolCallOutput;

pub async fn view_image(
    state: &crate::BrokerState,
    input: ViewImageInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = std::time::Instant::now();
    let target_name = input.target.clone();
    let detail = input.detail.clone();
    let path = input.path.clone();
    tracing::info!(
        tool = "view_image",
        target = %target_name,
        path = %path,
        detail = detail.as_deref().unwrap_or("default"),
        has_workdir = input.workdir.is_some(),
        "broker tool started"
    );
    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
    let response = match target
        .image_read(&ImageReadRequest {
            path: input.path,
            workdir: input.workdir,
            detail: input.detail.clone(),
        })
        .await
    {
        Ok(response) => response,
        Err(err) => {
            if matches!(err, DaemonClientError::Transport(_)) {
                target.clear_cached_daemon_info().await;
            }
            tracing::warn!(
                tool = "view_image",
                target = %target_name,
                path = %path,
                detail = detail.as_deref().unwrap_or("default"),
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "broker tool failed"
            );
            return Err(normalize_view_image_error(err));
        }
    };
    let image_content = content_from_data_url(&response.image_url)?;

    tracing::info!(
        tool = "view_image",
        target = %target_name,
        path = %path,
        detail = response.detail.as_deref().unwrap_or("default"),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );

    Ok(ToolCallOutput::content_and_structured(
        vec![image_content],
        serde_json::to_value(ViewImageResult {
            target: input.target,
            image_url: response.image_url,
            detail: response.detail,
        })?,
    ))
}

fn normalize_view_image_error(err: DaemonClientError) -> anyhow::Error {
    match err {
        DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}

fn content_from_data_url(image_url: &str) -> anyhow::Result<Content> {
    let (metadata, data) = image_url
        .split_once(',')
        .context("image read did not return a valid data URL")?;
    let mime_type = metadata
        .strip_prefix("data:")
        .and_then(|prefix| prefix.strip_suffix(";base64"))
        .context("image read did not return a base64 data URL")?;

    Ok(Content::image(data.to_string(), mime_type.to_string()))
}
