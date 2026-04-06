mod support;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use remote_exec_proto::rpc::{
    TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER,
    TRANSFER_SOURCE_TYPE_HEADER, TransferExportRequest, TransferImportResponse,
};

fn raw_tar_file_with_path(path: &Path, body: &[u8]) -> Vec<u8> {
    fn write_octal(field: &mut [u8], value: u64) {
        let digits = field.len() - 1;
        let text = format!("{value:o}");
        assert!(
            text.len() <= digits,
            "value {value} does not fit in tar field"
        );
        let start = digits - text.len();
        field[..start].fill(b'0');
        field[start..digits].copy_from_slice(text.as_bytes());
        field[digits] = 0;
    }

    fn write_checksum(field: &mut [u8], checksum: u32) {
        let text = format!("{checksum:o}");
        assert!(
            text.len() <= 6,
            "checksum {checksum} does not fit in tar field"
        );
        let start = 6 - text.len();
        field[..start].fill(b'0');
        field[start..6].copy_from_slice(text.as_bytes());
        field[6] = 0;
        field[7] = b' ';
    }

    let path = path.to_string_lossy();
    assert!(
        path.len() <= 100,
        "tar test helper only supports short paths"
    );
    let mut header = [0u8; 512];
    header[..path.len()].copy_from_slice(path.as_bytes());
    write_octal(&mut header[100..108], 0o644);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], body.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| *byte as u32).sum();
    write_checksum(&mut header[148..156], checksum);

    let mut archive = Vec::with_capacity(512 + body.len() + 1024);
    archive.extend_from_slice(&header);
    archive.extend_from_slice(body);
    let padding = (512 - (body.len() % 512)) % 512;
    archive.resize(archive.len() + padding, 0);
    archive.extend_from_slice(&[0u8; 1024]);
    archive
}

#[tokio::test]
async fn export_file_streams_archive_and_reports_file_source_type() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[cfg(unix)]
#[tokio::test]
async fn export_directory_rejects_nested_symlinks_before_streaming() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("app.txt"), "ok\n")
        .await
        .unwrap();
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

#[cfg(unix)]
#[tokio::test]
async fn export_rejects_symlink_source_root() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[cfg(unix)]
#[tokio::test]
async fn export_file_preserves_executable_mode_in_archive_header() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("tool.sh");
    tokio::fs::write(&source, "#!/bin/sh\necho hi\n")
        .await
        .unwrap();
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

#[cfg(windows)]
#[tokio::test]
async fn import_accepts_forward_slash_windows_destination_paths() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
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
    let destination = fixture.workdir.join("release").join("artifact.txt");
    let destination_text = destination.display().to_string().replace('\\', "/");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (TRANSFER_DESTINATION_PATH_HEADER, destination_text),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(
        tokio::fs::read_to_string(&destination).await.unwrap(),
        "artifact\n"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn export_accepts_msys_style_windows_source_paths() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("msys-source.txt");
    tokio::fs::write(&source, "artifact\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: support::msys_style_path(&source),
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

#[cfg(windows)]
#[tokio::test]
async fn import_accepts_cygwin_style_windows_destination_paths() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
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
    let destination = fixture
        .workdir
        .join("cygdrive-release")
        .join("artifact.txt");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    support::cygwin_style_path(&destination),
                ),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(
        tokio::fs::read_to_string(&destination).await.unwrap(),
        "artifact\n"
    );
}

#[tokio::test]
async fn import_directory_replaces_exact_destination_and_preserves_exec_bits() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();
    }

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
    #[cfg(unix)]
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
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
    assert_eq!(
        tokio::fs::read_to_string(&destination).await.unwrap(),
        "old\n"
    );
}

#[tokio::test]
async fn import_replaces_directory_with_file_at_the_exact_destination_path() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
    assert_eq!(
        tokio::fs::read_to_string(&destination).await.unwrap(),
        "artifact\n"
    );
}

#[tokio::test]
async fn import_rejects_missing_parent_when_create_parent_is_false() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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

#[tokio::test]
async fn import_rejects_directory_entries_that_escape_destination() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("release");
    let escaped = fixture.workdir.join("escaped.txt");
    let bytes = raw_tar_file_with_path(Path::new("../escaped.txt"), b"owned\n");

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

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_source_unsupported");
    assert!(
        err.message.contains("must not have `..`") || err.message.contains("unsupported entry")
    );
    assert!(!escaped.exists());
}

#[tokio::test]
async fn export_rejects_paths_outside_read_sandbox() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("visible").display().to_string(),
            )]);
            format!(
                r#"[sandbox.read]
allow = {allow}
"#
            )
        })
        .await;
    let blocked = fixture.workdir.join("blocked.txt");
    tokio::fs::write(&blocked, "blocked\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: blocked.display().to_string(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "sandbox_denied");
    assert!(err.message.contains("read access"));
}

#[tokio::test]
async fn import_rejects_destinations_outside_write_sandbox() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("allowed").display().to_string(),
            )]);
            format!(
                r#"[sandbox.write]
allow = {allow}
"#
            )
        })
        .await;
    let source = fixture.workdir.join("source.txt");
    let blocked_destination = fixture.workdir.join("blocked/out.txt");
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
                    blocked_destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
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
    assert_eq!(err.code, "sandbox_denied");
    assert!(err.message.contains("write access"));
    assert!(!blocked_destination.exists());
}
