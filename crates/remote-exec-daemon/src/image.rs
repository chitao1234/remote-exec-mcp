use std::fmt::Display;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ImageFormat};
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
    let path = normalize_path(&cwd.join(&req.path));
    let metadata = tokio::fs::metadata(&path).await.map_err(|err| {
        crate::exec::rpc_error(
            "image_missing",
            format!("unable to locate image at `{}`: {err}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(crate::exec::rpc_error(
            "image_not_file",
            format!("image path `{}` is not a file", path.display()),
        ));
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|err| process_error(&path, "image_decode_failed", err))?;
    let (format, rendered_bytes) = render_image_bytes(&path, req.detail.as_deref(), bytes)?;
    let image_url = encode_data_url(format, rendered_bytes)?;

    Ok(Json(ImageReadResponse {
        image_url,
        detail: req.detail.filter(|value| value == "original"),
    }))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn passthrough_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

fn output_format_for_processed_image(format: ImageFormat) -> ImageFormat {
    match format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::WebP => ImageFormat::WebP,
        _ => ImageFormat::Png,
    }
}

fn process_error(
    path: &Path,
    code: &'static str,
    err: impl Display,
) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error(
        code,
        format!("unable to process image at `{}`: {err}", path.display()),
    )
}

fn encode_processed_image(
    image: &DynamicImage,
    format: ImageFormat,
) -> Result<Vec<u8>, image::ImageError> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Png => image.write_to(&mut out, ImageFormat::Png)?,
        ImageFormat::Jpeg => {
            image.write_with_encoder(JpegEncoder::new_with_quality(&mut out, 85))?
        }
        ImageFormat::WebP => image.write_with_encoder(WebPEncoder::new_lossless(&mut out))?,
        other => unreachable!("unexpected processed image format: {other:?}"),
    }
    Ok(out.into_inner())
}

fn render_image_bytes(
    path: &Path,
    detail: Option<&str>,
    bytes: Vec<u8>,
) -> Result<(ImageFormat, Vec<u8>), (StatusCode, Json<RpcErrorBody>)> {
    let source_format = image::guess_format(&bytes)
        .map_err(|err| process_error(path, "image_decode_failed", err))?;
    let keep_original = detail == Some("original");
    if passthrough_format(source_format) && keep_original {
        return Ok((source_format, bytes));
    }

    let image = image::load_from_memory(&bytes)
        .map_err(|err| process_error(path, "image_decode_failed", err))?;
    let needs_resize = image.width() > MAX_WIDTH || image.height() > MAX_HEIGHT;
    if passthrough_format(source_format) && !needs_resize {
        return Ok((source_format, bytes));
    }

    let rendered = if keep_original || !needs_resize {
        image
    } else {
        image.resize(MAX_WIDTH, MAX_HEIGHT, image::imageops::FilterType::Triangle)
    };
    let output_format = output_format_for_processed_image(source_format);
    let rendered_bytes = encode_processed_image(&rendered, output_format)
        .map_err(|err| process_error(path, "image_encode_failed", err))?;
    Ok((output_format, rendered_bytes))
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
