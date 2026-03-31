use anyhow::Context;
use remote_exec_proto::public::{ViewImageInput, ViewImageResult};
use remote_exec_proto::rpc::ImageReadRequest;
use rmcp::model::Content;

use crate::mcp_server::ToolCallOutput;

pub async fn view_image(
    state: &crate::BrokerState,
    input: ViewImageInput,
) -> anyhow::Result<ToolCallOutput> {
    match input.detail.as_deref() {
        None | Some("original") => {}
        Some(other) => anyhow::bail!("view_image.detail only supports `original`; got `{other}`"),
    }

    let target = state.target(&input.target)?;
    let response = target
        .client
        .image_read(&ImageReadRequest {
            path: input.path,
            workdir: input.workdir,
            detail: input.detail.clone(),
        })
        .await?;
    let image_content = content_from_data_url(&response.image_url)?;

    Ok(ToolCallOutput::content_and_structured(
        vec![image_content],
        serde_json::to_value(ViewImageResult {
            target: input.target,
            image_url: response.image_url,
            detail: response.detail,
        })?,
    ))
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
