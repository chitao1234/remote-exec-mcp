mod support;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use base64::Engine;
use image::ImageFormat;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse};
use support::test_helpers::DEFAULT_TEST_TARGET;

async fn write_png(path: &Path, width: u32, height: u32) {
    write_image(path, width, height, ImageFormat::Png).await;
}

async fn write_image(path: &Path, width: u32, height: u32, format: ImageFormat) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save_with_format(path, format).unwrap();
}

async fn write_invalid_bytes(path: &Path) {
    tokio::fs::write(path, b"not an image").await.unwrap();
}

fn decode_data_url(image_url: &str) -> (String, Vec<u8>) {
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

async fn assert_default_passthrough(extension: &str, format: ImageFormat, expected_mime: &str) {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join(format!("small.{extension}"));
    write_image(&path, 64, 64, format).await;
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

    let (mime, returned) = decode_data_url(&response.image_url);
    assert_eq!(mime, expected_mime);
    assert_eq!(returned, original);
    assert_eq!(response.detail, None);
}

async fn assert_resized_output(
    extension: &str,
    format: ImageFormat,
    expected_mime: &str,
    width: u32,
    height: u32,
) {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join(format!("large.{extension}"));
    write_image(&path, width, height, format).await;

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: format!("large.{extension}"),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    let (mime, bytes) = decode_data_url(&response.image_url);
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!(mime, expected_mime);
    assert!(image.width() <= 2048);
    assert!(image.height() <= 2048);
    assert_eq!(response.detail, None);
}

#[tokio::test]
async fn image_read_preserves_large_passthrough_within_2048_square_threshold() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join("tall.webp");
    write_image(&path, 64, 2048, ImageFormat::WebP).await;
    let original = tokio::fs::read(&path).await.unwrap();

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "tall.webp".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    let (mime, returned) = decode_data_url(&response.image_url);
    assert_eq!(mime, "image/webp");
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
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join("original.webp");
    write_image(&path, 2050, 64, ImageFormat::WebP).await;
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

    let (mime, returned) = decode_data_url(&response.image_url);
    assert_eq!(mime, "image/webp");
    assert_eq!(returned, original);
    assert_eq!(response.detail, Some("original".to_string()));
}

#[tokio::test]
async fn image_read_resizes_large_png_and_keeps_png_encoding() {
    assert_resized_output("png", ImageFormat::Png, "image/png", 2050, 64).await;
}

#[tokio::test]
async fn image_read_resizes_large_jpeg_and_keeps_jpeg_encoding() {
    assert_resized_output("jpg", ImageFormat::Jpeg, "image/jpeg", 64, 2050).await;
}

#[tokio::test]
async fn image_read_reencodes_gif_in_default_mode() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join("anim.gif");
    write_image(&path, 64, 64, ImageFormat::Gif).await;
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

    let (mime, bytes) = decode_data_url(&response.image_url);
    assert_eq!(mime, "image/png");
    assert_ne!(bytes, original);
}

#[tokio::test]
async fn image_read_reports_missing_file_with_path_context() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;

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

    assert_eq!(err.wire_code(), "image_missing");
    assert!(err.message.contains("unable to locate image at"));
    assert!(err.message.contains("missing.png"));
}

#[cfg(windows)]
#[tokio::test]
async fn image_read_accepts_msys_style_absolute_paths_on_windows() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join("msys-path.png");
    write_png(&path, 48, 48).await;
    let original = tokio::fs::read(&path).await.unwrap();

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: support::msys_style_path(&path),
                workdir: None,
                detail: None,
            },
        )
        .await;

    let (mime, returned) = decode_data_url(&response.image_url);
    assert_eq!(mime, "image/png");
    assert_eq!(returned, original);
}

#[cfg(windows)]
#[tokio::test]
async fn image_read_accepts_windows_posix_root_paths_on_windows() {
    let fixture = support::spawn::spawn_daemon_with_extra_config_for_workdir(
        DEFAULT_TEST_TARGET,
        |workdir| {
            let root = workdir.join("synthetic-msys-root");
            format!(
                "windows_posix_root = {}\n",
                toml::Value::String(root.display().to_string())
            )
        },
    )
    .await;
    let root = fixture.workdir.join("synthetic-msys-root");
    let path = root.join("assets").join("synthetic-root.png");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    write_png(&path, 48, 48).await;
    let original = tokio::fs::read(&path).await.unwrap();

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "/assets/synthetic-root.png".to_string(),
                workdir: None,
                detail: None,
            },
        )
        .await;

    let (mime, returned) = decode_data_url(&response.image_url);
    assert_eq!(mime, "image/png");
    assert_eq!(returned, original);
}

#[tokio::test]
async fn image_read_rejects_directory_paths_with_path_context() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
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

    assert_eq!(err.wire_code(), "image_not_file");
    assert_eq!(
        err.message,
        format!("image path `{}` is not a file", dir.display())
    );
}

#[tokio::test]
async fn image_read_wraps_invalid_image_failures_with_path_context() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join("broken.png");
    write_invalid_bytes(&path).await;

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

    assert_eq!(err.wire_code(), "image_decode_failed");
    assert!(err.message.contains("unable to process image at"));
    assert!(err.message.contains("broken.png"));
}

#[tokio::test]
async fn image_read_rejects_unknown_detail_values() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let path = fixture.workdir.join("small.png");
    write_png(&path, 32, 32).await;

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

    assert_eq!(err.wire_code(), "invalid_detail");
    assert_eq!(
        err.message,
        "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`"
    );
}

#[tokio::test]
async fn image_read_uses_resolved_path_not_workdir_for_sandbox_checks() {
    let fixture = support::spawn::spawn_daemon_with_extra_config_for_workdir(
        DEFAULT_TEST_TARGET,
        |workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("visible").display().to_string(),
            )]);
            format!(
                r#"[sandbox.read]
allow = {allow}
"#
            )
        },
    )
    .await;
    let visible = fixture.workdir.join("visible");
    let hidden = fixture.workdir.join("hidden");
    tokio::fs::create_dir_all(&visible).await.unwrap();
    tokio::fs::create_dir_all(&hidden).await.unwrap();
    write_png(&visible.join("ok.png"), 16, 16).await;

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "../visible/ok.png".to_string(),
                workdir: Some("hidden".to_string()),
                detail: None,
            },
        )
        .await;

    assert!(response.image_url.starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn image_read_rejects_paths_outside_read_sandbox() {
    let fixture = support::spawn::spawn_daemon_with_extra_config_for_workdir(
        DEFAULT_TEST_TARGET,
        |workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("visible").display().to_string(),
            )]);
            format!(
                r#"[sandbox.read]
allow = {allow}
"#
            )
        },
    )
    .await;
    let hidden = fixture.workdir.join("hidden");
    tokio::fs::create_dir_all(&hidden).await.unwrap();
    write_png(&hidden.join("blocked.png"), 16, 16).await;

    let err = fixture
        .rpc_error(
            "/v1/image/read",
            &ImageReadRequest {
                path: "hidden/blocked.png".to_string(),
                workdir: None,
                detail: None,
            },
        )
        .await;

    assert_eq!(err.wire_code(), "sandbox_denied");
    assert!(err.message.contains("read access"));
}

#[cfg(unix)]
#[tokio::test]
async fn image_read_reports_permission_denied_as_internal_error() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    let hidden = fixture.workdir.join("hidden");
    tokio::fs::create_dir_all(&hidden).await.unwrap();
    write_png(&hidden.join("blocked.png"), 16, 16).await;

    let original_mode = std::fs::metadata(&hidden).unwrap().permissions().mode();
    let mut blocked_perms = std::fs::metadata(&hidden).unwrap().permissions();
    blocked_perms.set_mode(0o000);
    std::fs::set_permissions(&hidden, blocked_perms).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/image/read",
            &ImageReadRequest {
                path: "hidden/blocked.png".to_string(),
                workdir: None,
                detail: None,
            },
        )
        .await;

    let mut restored_perms = std::fs::metadata(&hidden).unwrap().permissions();
    restored_perms.set_mode(original_mode);
    std::fs::set_permissions(&hidden, restored_perms).unwrap();

    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.wire_code(), "internal_error");
    assert!(err.message.contains("blocked.png") || err.message.contains("Permission denied"));
}
