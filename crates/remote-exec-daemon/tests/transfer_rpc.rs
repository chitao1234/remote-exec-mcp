mod support;

use std::os::unix::fs::PermissionsExt;

use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportResponse, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
};

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

#[tokio::test]
async fn import_directory_replaces_exact_destination_and_preserves_exec_bits() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source_root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(source_root.join("empty"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(source_root.join("bin"))
        .await
        .unwrap();
    tokio::fs::write(source_root.join("bin/tool.sh"), "#!/bin/sh\necho hi\n")
        .await
        .unwrap();
    let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source_root.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();
    let destination = fixture.workdir.join("release");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    let summary = response.json::<TransferImportResponse>().await.unwrap();
    assert_eq!(
        summary.source_type,
        remote_exec_proto::rpc::TransferSourceType::Directory
    );
    assert_eq!(summary.files_copied, 1);
    assert_eq!(summary.directories_copied, 3);
    assert!(!summary.replaced);
    assert!(destination.join("empty").is_dir());
    assert_eq!(
        std::fs::metadata(destination.join("bin/tool.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
}

#[tokio::test]
async fn import_rejects_existing_destination_when_overwrite_is_fail() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
    let destination = fixture.workdir.join("dest.txt");
    tokio::fs::write(&source, "new\n").await.unwrap();
    tokio::fs::write(&destination, "old\n").await.unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_destination_exists");
    assert_eq!(tokio::fs::read_to_string(&destination).await.unwrap(), "old\n");
}

#[tokio::test]
async fn import_replaces_directory_with_file_at_the_exact_destination_path() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("tool.txt");
    let destination = fixture.workdir.join("release");
    tokio::fs::write(&source, "artifact\n").await.unwrap();
    tokio::fs::create_dir_all(destination.join("nested"))
        .await
        .unwrap();
    tokio::fs::write(destination.join("nested/old.txt"), "old\n")
        .await
        .unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(tokio::fs::read_to_string(&destination).await.unwrap(), "artifact\n");
}

#[tokio::test]
async fn import_rejects_missing_parent_when_create_parent_is_false() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
    let destination = fixture.workdir.join("missing/child.txt");
    tokio::fs::write(&source, "artifact\n").await.unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_parent_missing");
    assert!(!destination.exists());
}
