mod support;

use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse};

#[tokio::test]
async fn image_read_resizes_large_images_by_default() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("large.png");
    support::write_png(&path, 4096, 2048).await;

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "large.png".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    assert!(response.image_url.starts_with("data:image/png;base64,"));
    assert_eq!(response.detail, None);
}

#[tokio::test]
async fn image_read_rejects_unknown_detail_values() {
    let fixture = support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("small.png");
    support::write_png(&path, 32, 32).await;

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
    assert!(err.message.contains("original"));
}
