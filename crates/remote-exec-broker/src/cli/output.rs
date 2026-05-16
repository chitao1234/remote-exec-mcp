use std::path::Path;

use anyhow::Context;
use base64::Engine;

use crate::ToolResponse;

pub fn emit_response(response: &ToolResponse, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(response).context("serializing CLI response")?
        );
        return Ok(());
    }

    if response.is_error {
        if !response.text_output.is_empty() {
            eprintln!("{}", response.text_output);
        }
        return Ok(());
    }

    if !response.text_output.is_empty() {
        println!("{}", response.text_output);
    }

    Ok(())
}

pub fn emit_view_image_response(
    response: &ToolResponse,
    json: bool,
    output_path: Option<&Path>,
) -> anyhow::Result<()> {
    if json {
        return emit_response(response, true);
    }

    if response.is_error {
        return emit_response(response, false);
    }

    if let Some(path) = output_path {
        println!("Wrote image to {}", path.display());
        return Ok(());
    }

    if let Some(image_url) = response.first_image_url() {
        println!("{image_url}");
    }

    Ok(())
}

pub async fn write_image_output(response: &ToolResponse, out: &Path) -> anyhow::Result<()> {
    let image_url = response
        .first_image_url()
        .context("view_image response did not include an image payload")?;
    let bytes = decode_data_url(&image_url)?;
    tokio::fs::write(out, bytes)
        .await
        .with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

pub fn decode_data_url(image_url: &str) -> anyhow::Result<Vec<u8>> {
    let (metadata, payload) = image_url
        .split_once(',')
        .context("image payload was not a valid data URL")?;
    anyhow::ensure!(
        metadata.starts_with("data:") && metadata.ends_with(";base64"),
        "image payload was not a base64 data URL"
    );
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("decoding image data URL")
}
