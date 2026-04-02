mod support;

use std::os::unix::fs::PermissionsExt;

use remote_exec_proto::rpc::{TransferExportRequest, TRANSFER_SOURCE_TYPE_HEADER};

#[tokio::test]
async fn export_file_streams_archive_and_reports_file_source_type() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("hello.txt");
    tokio::fs::write(&source, "hello\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(
        response
            .headers()
            .get(TRANSFER_SOURCE_TYPE_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        "file"
    );
    assert!(!response.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn export_directory_rejects_nested_symlinks_before_streaming() {
    let fixture = support::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("app.txt"), "ok\n").await.unwrap();
    std::os::unix::fs::symlink(root.join("app.txt"), root.join("app-link")).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(body.code, "transfer_source_unsupported");
}

#[tokio::test]
async fn export_rejects_symlink_source_root() {
    let fixture = support::spawn_daemon("builder-a").await;
    let target = fixture.workdir.join("target.txt");
    let link = fixture.workdir.join("root-link");
    tokio::fs::write(&target, "ok\n").await.unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: link.display().to_string(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(body.code, "transfer_source_unsupported");
}

#[tokio::test]
async fn export_file_preserves_executable_mode_in_archive_header() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("tool.sh");
    tokio::fs::write(&source, "#!/bin/sh\necho hi\n").await.unwrap();
    let mut perms = std::fs::metadata(&source).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&source, perms).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = response.bytes().await.unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut entries = archive.entries().unwrap();
    let header = entries.next().unwrap().unwrap().header().clone();

    assert_eq!(header.mode().unwrap() & 0o111, 0o111);
}
