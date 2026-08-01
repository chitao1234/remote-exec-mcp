use anyhow::Context;
use remote_exec_proto::public::{ViewImageInput, ViewImageResult};
use remote_exec_proto::rpc::ImageReadRequest;
use rmcp::model::ContentBlock;

use crate::mcp_server::ToolCallOutput;

pub async fn view_image(
    state: &crate::BrokerState,
    input: ViewImageInput,
) -> anyhow::Result<ToolCallOutput> {
    crate::request_context::set_current_target(input.target.as_str());
    let path = input.path.clone();
    tracing::info!(
        tool = "view_image",
        target = %input.target,
        path = %path,
        detail = input.detail.as_deref().unwrap_or("default"),
        has_workdir = input.workdir.is_some(),
        "image read requested"
    );
    let response = state
        .image_read(
            &input.target,
            &ImageReadRequest {
                path: input.path,
                workdir: input.workdir,
                detail: input.detail,
            },
        )
        .await?;
    let image_content = content_from_data_url(&response.image_url)?;

    tracing::info!(
        tool = "view_image",
        target = %input.target,
        path = %path,
        detail = response.detail.as_deref().unwrap_or("default"),
        "image read completed"
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

fn content_from_data_url(image_url: &str) -> anyhow::Result<ContentBlock> {
    let (metadata, data) = image_url
        .split_once(',')
        .context("image read did not return a valid data URL")?;
    let mime_type = metadata
        .strip_prefix("data:")
        .and_then(|prefix| prefix.strip_suffix(";base64"))
        .context("image read did not return a base64 data URL")?;

    Ok(ContentBlock::image(data.to_owned(), mime_type.to_owned()))
}
