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
use remote_exec_proto::sandbox::SandboxAccess;

use crate::AppState;
use crate::error::ImageError;

const MAX_WIDTH: u32 = 2048;
const MAX_HEIGHT: u32 = 2048;

pub async fn read_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    read_image_local(state, req)
        .await
        .map(Json)
        .map_err(ImageError::into_rpc)
}

pub async fn read_image_local(
    state: Arc<AppState>,
    req: ImageReadRequest,
) -> Result<ImageReadResponse, ImageError> {
    tracing::info!(
        target = %state.config.target,
        path = %req.path,
        detail = req.detail.as_deref().unwrap_or("default"),
        has_workdir = req.workdir.is_some(),
        "image_read received"
    );
    match req.detail.as_deref() {
        None | Some("original") => {}
        Some(other) => {
            return Err(ImageError::invalid_detail(format!(
                "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `{other}`"
            )));
        }
    }

    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref())
        .map_err(|err| ImageError::internal(err.to_string()))?;
    let path = normalize_path(&crate::exec::resolve_input_path_with_windows_posix_root(
        &cwd,
        &req.path,
        state.config.windows_posix_root.as_deref(),
    ));
    crate::exec::ensure_sandbox_access(&state, SandboxAccess::Read, &path)
        .map_err(|err| ImageError::sandbox_denied(err.to_string()))?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|err| {
        ImageError::missing(format!(
            "unable to locate image at `{}`: {err}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(ImageError::not_file(format!(
            "image path `{}` is not a file",
            path.display()
        )));
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|err| process_error(&path, err))?;
    let (format, rendered_bytes) = render_image_bytes(&path, req.detail.as_deref(), bytes)?;
    let image_url = encode_data_url(format, rendered_bytes)?;
    tracing::info!(
        target = %state.config.target,
        path = %path.display(),
        detail = req.detail.as_deref().unwrap_or("default"),
        image_url_len = image_url.len(),
        "image_read completed"
    );

    Ok(ImageReadResponse {
        image_url,
        detail: req.detail.filter(|value| value == "original"),
    })
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

fn process_error(path: &Path, err: impl Display) -> ImageError {
    ImageError::decode_failed(format!(
        "unable to process image at `{}`: {err}",
        path.display()
    ))
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
) -> Result<(ImageFormat, Vec<u8>), ImageError> {
    let source_format = image::guess_format(&bytes).map_err(|err| process_error(path, err))?;
    let keep_original = detail == Some("original");
    if passthrough_format(source_format) && keep_original {
        return Ok((source_format, bytes));
    }

    let image = image::load_from_memory(&bytes).map_err(|err| process_error(path, err))?;
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
    let rendered_bytes =
        encode_processed_image(&rendered, output_format).map_err(|err| process_error(path, err))?;
    Ok((output_format, rendered_bytes))
}

fn encode_data_url(format: ImageFormat, bytes: Vec<u8>) -> Result<String, ImageError> {
    let mime = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        other => {
            return Err(ImageError::decode_failed(format!(
                "unsupported image format `{other:?}`"
            )));
        }
    };

    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
