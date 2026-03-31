use std::io::Cursor;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use base64::Engine;
use image::ImageFormat;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse, RpcErrorBody};

use crate::AppState;

const MAX_WIDTH: u32 = 2048;
const MAX_HEIGHT: u32 = 768;

pub async fn read_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    match req.detail.as_deref() {
        None | Some("original") => {}
        Some(other) => {
            return Err(crate::exec::rpc_error(
                "invalid_detail",
                format!(
                    "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `{other}`"
                ),
            ));
        }
    }

    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)?;
    let path = cwd.join(&req.path);
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|err| crate::exec::rpc_error("image_missing", err.to_string()))?;
    if !metadata.is_file() {
        return Err(crate::exec::rpc_error(
            "image_not_file",
            format!("image path `{}` is not a file", path.display()),
        ));
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|err| crate::exec::internal_error(err.into()))?;
    let format = image::guess_format(&bytes)
        .map_err(|err| crate::exec::rpc_error("image_decode_failed", err.to_string()))?;
    let image_url = if req.detail.as_deref() == Some("original") {
        encode_data_url(format, bytes)?
    } else {
        let image = image::load_from_memory(&bytes)
            .map_err(|err| crate::exec::rpc_error("image_decode_failed", err.to_string()))?;
        let resized = image.resize(
            MAX_WIDTH,
            MAX_HEIGHT,
            image::imageops::FilterType::Triangle,
        );
        let mut out = Vec::new();
        resized
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .map_err(|err| crate::exec::rpc_error("image_encode_failed", err.to_string()))?;
        encode_data_url(ImageFormat::Png, out)?
    };

    Ok(Json(ImageReadResponse {
        image_url,
        detail: req.detail.filter(|value| value == "original"),
    }))
}

fn encode_data_url(
    format: ImageFormat,
    bytes: Vec<u8>,
) -> Result<String, (StatusCode, Json<RpcErrorBody>)> {
    let mime = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        other => {
            return Err(crate::exec::rpc_error(
                "image_decode_failed",
                format!("unsupported image format `{other:?}`"),
            ));
        }
    };

    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
