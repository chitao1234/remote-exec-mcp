# View Image Processing And Error Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align `view_image` processing, encoding, and visible error behavior with the approved parity batch while preserving the existing broker-facing schema and always-exposed tool policy.

**Architecture:** Keep image decoding, resize decisions, and path-context error construction in the daemon so machine-local filesystem behavior stays local. Keep the broker responsible for target routing and MCP wrapping, but stop it from inventing its own `detail` rejection text or leaking daemon RPC wrapper prefixes into `view_image` user-facing errors.

**Tech Stack:** Rust 2024, Tokio integration tests, `axum`, `reqwest`, `rmcp`, `image` 0.25, `base64`

---

## File Map

- `crates/remote-exec-daemon/src/image.rs`
  Responsibility: implement default-vs-original processing branches, passthrough/re-encode decisions, and Codex-style path-context error messages.
- `crates/remote-exec-daemon/tests/support/mod.rs`
  Responsibility: create multi-format fixtures and decode returned data URLs for daemon integration tests.
- `crates/remote-exec-daemon/tests/image_rpc.rs`
  Responsibility: prove daemon-visible processing, encoding, and error wording behavior.
- `crates/remote-exec-broker/src/tools/image.rs`
  Responsibility: forward `detail` validation to the daemon and surface daemon RPC image errors as plain messages in the MCP tool result.
- `crates/remote-exec-broker/tests/support/mod.rs`
  Responsibility: make the stub daemon image endpoint configurable for both success and failure paths.
- `crates/remote-exec-broker/tests/mcp_assets.rs`
  Responsibility: prove broker-visible `input_image` success behavior and text-only failure behavior for `view_image`.

### Task 1: Daemon Processing And Error Wording

**Files:**
- Modify: `crates/remote-exec-daemon/src/image.rs`
- Modify: `crates/remote-exec-daemon/tests/support/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/image_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test image_rpc`

**Testing approach:** `TDD`
Reason: the daemon already has a stable `ImageReadRequest` / `ImageReadResponse` seam, and the approved batch is defined almost entirely in terms of observable MIME, bytes, resize behavior, and error text.

- [ ] **Step 1: Expand daemon fixtures and write the failing daemon parity tests**

```rust
// crates/remote-exec-daemon/tests/support/mod.rs
use base64::Engine;
use image::ImageFormat;

pub async fn write_image(path: &Path, width: u32, height: u32, format: ImageFormat) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save_with_format(path, format).unwrap();
}

pub async fn write_invalid_bytes(path: &Path) {
    tokio::fs::write(path, b"not an image").await.unwrap();
}

pub fn decode_data_url(image_url: &str) -> (String, Vec<u8>) {
    let (metadata, data) = image_url.split_once(',').unwrap();
    let mime = metadata
        .strip_prefix("data:")
        .unwrap()
        .strip_suffix(";base64")
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    (mime.to_string(), bytes)
}
```

```rust
// crates/remote-exec-daemon/tests/image_rpc.rs
use image::ImageFormat;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse};

async fn assert_default_passthrough(
    extension: &str,
    format: ImageFormat,
    expected_mime: &str,
) {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join(format!("small.{extension}"));
    support::write_image(&path, 64, 64, format).await;
    let original = tokio::fs::read(&path).await.unwrap();

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: format!("small.{extension}"),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    let (mime, returned) = support::decode_data_url(&response.image_url);
    assert_eq!(mime, expected_mime);
    assert_eq!(returned, original);
    assert_eq!(response.detail, None);
}

#[tokio::test]
async fn image_read_preserves_small_png_jpeg_and_webp_bytes_by_default() {
    assert_default_passthrough("png", ImageFormat::Png, "image/png").await;
    assert_default_passthrough("jpg", ImageFormat::Jpeg, "image/jpeg").await;
    assert_default_passthrough("webp", ImageFormat::WebP, "image/webp").await;
}

#[tokio::test]
async fn image_read_preserves_original_detail_for_passthrough_formats() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("original.webp");
    support::write_image(&path, 3000, 2000, ImageFormat::WebP).await;
    let original = tokio::fs::read(&path).await.unwrap();

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "original.webp".to_string(),
                workdir: Some(".".to_string()),
                detail: Some("original".to_string()),
            },
        )
        .await;

    let (mime, returned) = support::decode_data_url(&response.image_url);
    assert_eq!(mime, "image/webp");
    assert_eq!(returned, original);
    assert_eq!(response.detail, Some("original".to_string()));
}

#[tokio::test]
async fn image_read_resizes_large_jpeg_and_keeps_jpeg_encoding() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("large.jpg");
    support::write_image(&path, 4096, 2048, ImageFormat::Jpeg).await;

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "large.jpg".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    let (mime, bytes) = support::decode_data_url(&response.image_url);
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!(mime, "image/jpeg");
    assert!(image.width() <= 2048);
    assert!(image.height() <= 768);
}

#[tokio::test]
async fn image_read_reencodes_gif_in_default_mode() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("anim.gif");
    support::write_image(&path, 64, 64, ImageFormat::Gif).await;
    let original = tokio::fs::read(&path).await.unwrap();

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "anim.gif".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    let (mime, bytes) = support::decode_data_url(&response.image_url);
    assert_eq!(mime, "image/png");
    assert_ne!(bytes, original);
}

#[tokio::test]
async fn image_read_reports_missing_file_with_path_context() {
    let fixture = support::spawn_daemon("builder-a").await;

    let err = fixture
        .rpc_error(
            "/v1/image/read",
            &ImageReadRequest {
                path: "missing.png".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    assert_eq!(err.code, "image_missing");
    assert!(err.message.contains("unable to locate image at"));
    assert!(err.message.contains("missing.png"));
}
```

```rust
// crates/remote-exec-daemon/tests/image_rpc.rs
#[tokio::test]
async fn image_read_rejects_directory_paths_with_path_context() {
    let fixture = support::spawn_daemon("builder-a").await;
    let dir = fixture.workdir.join("nested");
    tokio::fs::create_dir_all(&dir).await.unwrap();

    let err = fixture
        .rpc_error(
            "/v1/image/read",
            &ImageReadRequest {
                path: "nested".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    assert_eq!(err.code, "image_not_file");
    assert_eq!(
        err.message,
        format!("image path `{}` is not a file", dir.display())
    );
}

#[tokio::test]
async fn image_read_wraps_invalid_image_failures_with_path_context() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("broken.png");
    support::write_invalid_bytes(&path).await;

    let err = fixture
        .rpc_error(
            "/v1/image/read",
            &ImageReadRequest {
                path: "broken.png".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    assert_eq!(err.code, "image_decode_failed");
    assert!(err.message.contains("unable to process image at"));
    assert!(err.message.contains("broken.png"));
}

#[tokio::test]
async fn image_read_rejects_unknown_detail_values_with_full_message() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("small.png");
    support::write_image(&path, 32, 32, ImageFormat::Png).await;

    let err = fixture
        .rpc_error(
            "/v1/image/read",
            &ImageReadRequest {
                path: "small.png".to_string(),
                workdir: Some(".".to_string()),
                detail: Some("low".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "invalid_detail");
    assert_eq!(
        err.message,
        "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`"
    );
}
```

- [ ] **Step 2: Run the daemon-focused suite and confirm the new tests fail before implementation**

Run: `cargo test -p remote-exec-daemon --test image_rpc`
Expected: FAIL. At minimum, the new small JPEG/WebP passthrough assertions should see `data:image/png` instead of the original MIME, and the missing/invalid-image error assertions should still see raw filesystem or decoder text without the `unable to locate image at <path>` or `unable to process image at <path>` wrapper.

- [ ] **Step 3: Implement the daemon processing branches and error wrappers**

```rust
// crates/remote-exec-daemon/src/image.rs
use std::fmt::Display;
use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ImageFormat};

fn passthrough_format(format: ImageFormat) -> bool {
    matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP)
}

fn output_format_for_processed_image(format: ImageFormat) -> ImageFormat {
    match format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::WebP => ImageFormat::WebP,
        _ => ImageFormat::Png,
    }
}

fn process_error(path: &Path, code: &'static str, err: impl Display) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error(
        code,
        format!("unable to process image at `{}`: {err}", path.display()),
    )
}

fn encode_processed_image(image: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>, image::ImageError> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Png => image.write_to(&mut out, ImageFormat::Png)?,
        ImageFormat::Jpeg => image.write_with_encoder(JpegEncoder::new_with_quality(&mut out, 85))?,
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
    let out = encode_processed_image(&rendered, output_format)
        .map_err(|err| process_error(path, "image_encode_failed", err))?;
    Ok((output_format, out))
}

let path = cwd.join(&req.path);
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
let (output_format, output_bytes) = render_image_bytes(&path, req.detail.as_deref(), bytes)?;
let image_url = encode_data_url(output_format, output_bytes)?;
```

```rust
// crates/remote-exec-daemon/src/image.rs
let response_detail = req.detail.filter(|value| value == "original");
Ok(Json(ImageReadResponse {
    image_url,
    detail: response_detail,
}))
```

- [ ] **Step 4: Run the daemon-focused suite again**

Run: `cargo test -p remote-exec-daemon --test image_rpc`
Expected: PASS. The suite should confirm default small PNG/JPEG/WebP passthrough, `"original"` passthrough preservation, JPEG resize re-encoding, GIF default re-encoding, and the new path-context error wording.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/image.rs crates/remote-exec-daemon/tests/support/mod.rs crates/remote-exec-daemon/tests/image_rpc.rs
git commit -m "feat: align daemon view_image processing"
```

### Task 2: Broker View Image Error Surface

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/image.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets`

**Testing approach:** `characterization/integration test`
Reason: the broker-facing contract is about MCP content shape and error text after daemon transport wrapping, so the right seam is a public `view_image` tool call against a configurable stub daemon rather than isolated unit tests.

- [ ] **Step 1: Make the stub image endpoint configurable and add failing broker tests**

```rust
// crates/remote-exec-broker/tests/support/mod.rs
#[derive(Debug, Clone)]
pub enum StubImageReadResponse {
    Success(ImageReadResponse),
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Clone)]
struct StubDaemonState {
    target: String,
    daemon_instance_id: String,
    fail_exec_write_once: Arc<Mutex<bool>>,
    exec_start_calls: Arc<Mutex<usize>>,
    last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    image_read_response: Arc<Mutex<StubImageReadResponse>>,
}

fn stub_daemon_state(target: &str, fail_exec_write_once: bool) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: "daemon-instance-1".to_string(),
        fail_exec_write_once: Arc::new(Mutex::new(fail_exec_write_once)),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
    }
}

impl BrokerFixture {
    pub async fn raw_tool_result(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        self.raw_call_tool(name, arguments).await
    }

    pub async fn set_image_read_response(&self, response: StubImageReadResponse) {
        *self.stub_state.image_read_response.lock().await = response;
    }
}

async fn image_read(
    State(state): State<StubDaemonState>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    match state.image_read_response.lock().await.clone() {
        StubImageReadResponse::Success(mut response) => {
            response.detail = req.detail.filter(|value| value == "original");
            Ok(Json(response))
        }
        StubImageReadResponse::Error { status, body } => Err((status, Json(body))),
    }
}
```

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs
use axum::http::StatusCode;
use remote_exec_proto::rpc::RpcErrorBody;

#[tokio::test]
async fn view_image_returns_text_only_errors_without_input_image_content() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    fixture
        .set_image_read_response(support::StubImageReadResponse::Error {
            status: StatusCode::BAD_REQUEST,
            body: RpcErrorBody {
                code: "image_missing".to_string(),
                message: "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)".to_string(),
            },
        })
        .await;

    let result = fixture
        .raw_tool_result(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png"
            }),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(
        result.text_output,
        "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)"
    );
    assert_eq!(
        result.raw_content,
        vec![serde_json::json!({
            "type": "text",
            "text": "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)"
        })]
    );
}

#[tokio::test]
async fn view_image_invalid_detail_matches_daemon_message() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    fixture
        .set_image_read_response(support::StubImageReadResponse::Error {
            status: StatusCode::BAD_REQUEST,
            body: RpcErrorBody {
                code: "invalid_detail".to_string(),
                message: "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`".to_string(),
            },
        })
        .await;

    let result = fixture
        .raw_tool_result(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png",
                "detail": "low"
            }),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(
        result.text_output,
        "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`"
    );
}
```

- [ ] **Step 2: Run the broker-focused suite and confirm the new tests fail before implementation**

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: FAIL. The invalid-detail test should still fail because the broker rejects `"low"` before calling the daemon, and the missing-file test should still see the daemon code/status wrapper `image_missing:` plus `(400 Bad Request)` instead of the plain daemon message.

- [ ] **Step 3: Remove broker-side detail prevalidation and normalize daemon RPC image errors to plain messages**

```rust
// crates/remote-exec-broker/src/tools/image.rs
pub async fn view_image(
    state: &crate::BrokerState,
    input: ViewImageInput,
) -> anyhow::Result<ToolCallOutput> {
    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
    let response = target
        .client
        .image_read(&ImageReadRequest {
            path: input.path,
            workdir: input.workdir,
            detail: input.detail.clone(),
        })
        .await
        .map_err(normalize_view_image_error)?;
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

fn normalize_view_image_error(err: crate::daemon_client::DaemonClientError) -> anyhow::Error {
    match err {
        crate::daemon_client::DaemonClientError::Rpc { message, .. } => anyhow::anyhow!(message),
        other => other.into(),
    }
}
```

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs
#[tokio::test]
async fn view_image_returns_input_image_content_and_structured_content() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png",
                "detail": "original"
            }),
        )
        .await;

    assert_eq!(result.raw_content[0]["type"], "input_image");
    assert_eq!(result.raw_content[0]["image_url"], "data:image/png;base64,AAAA");
    assert_eq!(result.structured_content["target"], "builder-a");
    assert_eq!(result.structured_content["detail"], "original");
}
```

- [ ] **Step 4: Run the broker-focused suite again**

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: PASS. The suite should confirm the existing `input_image` success shape and the new plain-text failure shape with no image content item on error.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/image.rs crates/remote-exec-broker/tests/support/mod.rs crates/remote-exec-broker/tests/mcp_assets.rs
git commit -m "feat: align broker view_image errors"
```

## Final Verification

- Run: `cargo test -p remote-exec-daemon --test image_rpc`
  Expected: PASS with the new daemon processing and error wording coverage.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
  Expected: PASS with `input_image` success and text-only error coverage.
- Run: `cargo test --workspace`
  Expected: PASS for the full workspace after both tasks land.
- Run: `cargo fmt --all --check`
  Expected: PASS with no formatting drift.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  Expected: PASS with no lint regressions.
