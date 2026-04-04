mod support;

use image::ImageFormat;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse};

async fn assert_default_passthrough(extension: &str, format: ImageFormat, expected_mime: &str) {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join(format!("small.{extension}"));
    support::assets::write_image(&path, 64, 64, format).await;
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

    let (mime, returned) = support::assets::decode_data_url(&response.image_url);
    assert_eq!(mime, expected_mime);
    assert_eq!(returned, original);
    assert_eq!(response.detail, None);
}

async fn assert_resized_output(extension: &str, format: ImageFormat, expected_mime: &str) {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join(format!("large.{extension}"));
    support::assets::write_image(&path, 4096, 2048, format).await;

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

    let (mime, bytes) = support::assets::decode_data_url(&response.image_url);
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!(mime, expected_mime);
    assert!(image.width() <= 2048);
    assert!(image.height() <= 768);
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
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("original.webp");
    support::assets::write_image(&path, 3000, 2000, ImageFormat::WebP).await;
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

    let (mime, returned) = support::assets::decode_data_url(&response.image_url);
    assert_eq!(mime, "image/webp");
    assert_eq!(returned, original);
    assert_eq!(response.detail, Some("original".to_string()));
}

#[tokio::test]
async fn image_read_resizes_large_png_and_keeps_png_encoding() {
    assert_resized_output("png", ImageFormat::Png, "image/png").await;
}

#[tokio::test]
async fn image_read_resizes_large_jpeg_and_keeps_jpeg_encoding() {
    assert_resized_output("jpg", ImageFormat::Jpeg, "image/jpeg").await;
}

#[tokio::test]
async fn image_read_reencodes_gif_in_default_mode() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("anim.gif");
    support::assets::write_image(&path, 64, 64, ImageFormat::Gif).await;
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

    let (mime, bytes) = support::assets::decode_data_url(&response.image_url);
    assert_eq!(mime, "image/png");
    assert_ne!(bytes, original);
}

#[tokio::test]
async fn image_read_reports_missing_file_with_path_context() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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

#[tokio::test]
async fn image_read_rejects_directory_paths_with_path_context() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("broken.png");
    support::assets::write_invalid_bytes(&path).await;

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
async fn image_read_rejects_unknown_detail_values() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("small.png");
    support::assets::write_png(&path, 32, 32).await;

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
