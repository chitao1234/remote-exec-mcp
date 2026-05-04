mod support;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use remote_exec_proto::rpc::{
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER, TransferCompression,
    TransferExportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse, TransferSourceType,
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

fn multi_source_tar() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    let file_body = b"alpha\n";
    let mut alpha = tar::Header::new_gnu();
    alpha.set_entry_type(tar::EntryType::Regular);
    alpha.set_mode(0o644);
    alpha.set_size(file_body.len() as u64);
    alpha.set_cksum();
    builder
        .append_data(
            &mut alpha,
            "alpha.txt",
            std::io::Cursor::new(file_body.as_slice()),
        )
        .unwrap();

    let mut nested = tar::Header::new_gnu();
    nested.set_entry_type(tar::EntryType::Directory);
    nested.set_mode(0o755);
    nested.set_size(0);
    nested.set_cksum();
    builder
        .append_data(&mut nested, "nested", std::io::empty())
        .unwrap();

    let nested_body = b"beta\n";
    let mut beta = tar::Header::new_gnu();
    beta.set_entry_type(tar::EntryType::Regular);
    beta.set_mode(0o644);
    beta.set_size(nested_body.len() as u64);
    beta.set_cksum();
    builder
        .append_data(
            &mut beta,
            "nested/beta.txt",
            std::io::Cursor::new(nested_body.as_slice()),
        )
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap()
}

#[cfg(unix)]
fn directory_tar_with_symlink() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    let file_body = b"alpha\n";
    let mut alpha = tar::Header::new_gnu();
    alpha.set_entry_type(tar::EntryType::Regular);
    alpha.set_mode(0o644);
    alpha.set_size(file_body.len() as u64);
    alpha.set_cksum();
    builder
        .append_data(
            &mut alpha,
            "alpha.txt",
            std::io::Cursor::new(file_body.as_slice()),
        )
        .unwrap();

    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    builder
        .append_link(&mut link, "alpha-link", "alpha.txt")
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap()
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
async fn transfer_path_info_reports_existing_directory() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("release");
    tokio::fs::create_dir_all(&destination).await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/path-info",
            &TransferPathInfoRequest {
                path: destination.display().to_string(),
            },
        )
        .await;

    assert!(response.status().is_success());
    let info = response.json::<TransferPathInfoResponse>().await.unwrap();
    assert!(info.exists);
    assert!(info.is_directory);
}

#[tokio::test]
async fn transfer_path_info_reports_missing_destination() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("missing");

    let response = fixture
        .raw_post_json(
            "/v1/transfer/path-info",
            &TransferPathInfoRequest {
                path: destination.display().to_string(),
            },
        )
        .await;

    assert!(response.status().is_success());
    let info = response.json::<TransferPathInfoResponse>().await.unwrap();
    assert!(!info.exists);
    assert!(!info.is_directory);
}

#[tokio::test]
async fn transfer_path_info_rejects_relative_paths_with_explicit_code() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = fixture
        .raw_post_json(
            "/v1/transfer/path-info",
            &TransferPathInfoRequest {
                path: "relative/output".to_string(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_path_not_absolute");
    assert!(err.message.contains("relative/output"));
}

#[tokio::test]
async fn export_file_supports_zstd_compression() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("hello.txt");
    tokio::fs::write(&source, "hello\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
                compression: TransferCompression::Zstd,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(
        response
            .headers()
            .get(TRANSFER_COMPRESSION_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        "zstd"
    );
    let decoded =
        zstd::stream::decode_all(std::io::Cursor::new(response.bytes().await.unwrap())).unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(decoded));
    let mut entries = archive.entries().unwrap();
    let entry = entries.next().unwrap().unwrap();
    assert_eq!(
        entry.path().unwrap().as_ref(),
        Path::new(".remote-exec-file")
    );
}

#[tokio::test]
async fn export_reports_missing_sources_with_explicit_code() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let missing = fixture.workdir.join("missing.txt");

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: missing.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_source_missing");
    assert!(err.message.contains("missing.txt"));
}

#[cfg(unix)]
#[tokio::test]
async fn export_directory_preserves_symlinks_by_default() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("app.txt"), "ok\n")
        .await
        .unwrap();
    std::os::unix::fs::symlink("app.txt", root.join("app-link")).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;

    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap().to_vec();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let symlink = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| entry.path().unwrap().as_ref() == Path::new("app-link"))
        .expect("symlink archive entry");
    assert!(symlink.header().entry_type().is_symlink());
    assert_eq!(
        symlink.link_name().unwrap().unwrap().as_ref(),
        Path::new("app.txt")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn export_directory_skips_special_files_with_warning() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("app.txt"), "ok\n")
        .await
        .unwrap();
    let fifo = root.join("events.fifo");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;

    assert!(response.status().is_success());
    assert!(
        response
            .headers()
            .get("x-remote-exec-warnings-bin")
            .is_none()
    );

    let bytes = response.bytes().await.unwrap().to_vec();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let paths = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().to_path_buf())
        .collect::<Vec<_>>();
    assert!(paths.iter().any(|path| path == Path::new("app.txt")));
    assert!(!paths.iter().any(|path| path == Path::new("events.fifo")));

    let destination = fixture.workdir.join("imported");
    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "merge".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
            ],
            archive.into_inner().into_inner(),
        )
        .await;

    assert!(response.status().is_success());
    let summary = response.json::<TransferImportResponse>().await.unwrap();
    assert_eq!(summary.warnings.len(), 1);
    assert_eq!(
        summary.warnings[0].code,
        "transfer_skipped_unsupported_entry"
    );
    assert!(
        !destination
            .join(".remote-exec-transfer-summary.json")
            .exists()
    );
}

#[tokio::test]
async fn export_directory_excludes_matching_entries_relative_to_source_root() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(root.join(".git")).await.unwrap();
    tokio::fs::create_dir_all(root.join("logs")).await.unwrap();
    tokio::fs::create_dir_all(root.join("src")).await.unwrap();
    tokio::fs::write(root.join("keep.txt"), "keep\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("top.log"), "drop\n")
        .await
        .unwrap();
    tokio::fs::write(root.join(".git/config"), "secret\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("logs/readme.txt"), "keep\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("logs/app.log"), "drop\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("src/a.rs"), "drop\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("src/z.rs"), "keep\n")
        .await
        .unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: vec![
                    "**/*.log".to_string(),
                    ".git/**".to_string(),
                    "src/[ab].rs".to_string(),
                ],
            },
        )
        .await;

    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap().to_vec();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let paths = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().to_path_buf())
        .collect::<Vec<_>>();

    assert!(paths.iter().any(|path| path == Path::new(".")));
    assert!(paths.iter().any(|path| path == Path::new("keep.txt")));
    assert!(
        paths
            .iter()
            .any(|path| path == Path::new("logs/readme.txt"))
    );
    assert!(paths.iter().any(|path| path == Path::new("src/z.rs")));
    assert!(!paths.iter().any(|path| path == Path::new("top.log")));
    assert!(!paths.iter().any(|path| path == Path::new(".git")));
    assert!(!paths.iter().any(|path| path == Path::new(".git/config")));
    assert!(!paths.iter().any(|path| path == Path::new("logs/app.log")));
    assert!(!paths.iter().any(|path| path == Path::new("src/a.rs")));
}

#[tokio::test]
async fn export_single_file_ignores_exclude_patterns() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("hello.txt");
    tokio::fs::write(&source, "hello\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: vec!["**/*.txt".to_string()],
            },
        )
        .await;

    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut entries = archive.entries().unwrap();
    let entry = entries.next().unwrap().unwrap();
    assert_eq!(
        entry.path().unwrap().as_ref(),
        Path::new(".remote-exec-file")
    );
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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

#[cfg(windows)]
#[tokio::test]
async fn export_accepts_windows_posix_root_source_paths() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let root = workdir.join("synthetic-msys-root");
            format!(
                "windows_posix_root = {}\n",
                toml::Value::String(root.display().to_string())
            )
        })
        .await;
    let root = fixture.workdir.join("synthetic-msys-root");
    let source = root.join("artifacts").join("synthetic-source.txt");
    tokio::fs::create_dir_all(source.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&source, "artifact\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: "/artifacts/synthetic-source.txt".to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;

    assert!(response.status().is_success());
    assert!(!response.bytes().await.unwrap().is_empty());
}

#[cfg(windows)]
#[tokio::test]
async fn import_accepts_windows_posix_root_destination_paths() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let root = workdir.join("synthetic-msys-root");
            format!(
                "windows_posix_root = {}\n",
                toml::Value::String(root.display().to_string())
            )
        })
        .await;
    let source = fixture.workdir.join("source.txt");
    tokio::fs::write(&source, "artifact\n").await.unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();
    let root = fixture.workdir.join("synthetic-msys-root");
    let destination = root.join("release").join("artifact.txt");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    "/release/artifact.txt".to_string(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
async fn import_multiple_source_archive_creates_destination_directory_bundle() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("bundle");

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
                (TRANSFER_SOURCE_TYPE_HEADER, "multiple".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
            ],
            multi_source_tar(),
        )
        .await;

    assert!(response.status().is_success());
    let summary = response.json::<TransferImportResponse>().await.unwrap();
    assert_eq!(summary.source_type, TransferSourceType::Multiple);
    assert_eq!(summary.files_copied, 2);
    assert_eq!(summary.directories_copied, 2);
    assert_eq!(
        tokio::fs::read_to_string(destination.join("alpha.txt"))
            .await
            .unwrap(),
        "alpha\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(destination.join("nested/beta.txt"))
            .await
            .unwrap(),
        "beta\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn import_directory_preserves_symlinks_by_default() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
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
                (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
            ],
            directory_tar_with_symlink(),
        )
        .await;

    assert!(response.status().is_success());
    let summary = response.json::<TransferImportResponse>().await.unwrap();
    assert_eq!(summary.files_copied, 2);
    assert_eq!(
        tokio::fs::read_to_string(destination.join("alpha.txt"))
            .await
            .unwrap(),
        "alpha\n"
    );
    assert_eq!(
        tokio::fs::read_link(destination.join("alpha-link"))
            .await
            .unwrap(),
        Path::new("alpha.txt")
    );
}

#[tokio::test]
async fn import_directory_merge_preserves_unrelated_destination_entries() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source_root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(source_root.join("nested"))
        .await
        .unwrap();
    tokio::fs::write(source_root.join("nested/new.txt"), "new\n")
        .await
        .unwrap();
    let destination = fixture.workdir.join("release");
    tokio::fs::create_dir_all(destination.join("nested"))
        .await
        .unwrap();
    tokio::fs::write(destination.join("keep.txt"), "keep\n")
        .await
        .unwrap();
    tokio::fs::write(destination.join("nested/old.txt"), "old\n")
        .await
        .unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source_root.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                (TRANSFER_OVERWRITE_HEADER, "merge".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    let summary = response.json::<TransferImportResponse>().await.unwrap();
    assert_eq!(summary.source_type, TransferSourceType::Directory);
    assert!(!summary.replaced);
    assert_eq!(
        tokio::fs::read_to_string(destination.join("nested/new.txt"))
            .await
            .unwrap(),
        "new\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(destination.join("keep.txt"))
            .await
            .unwrap(),
        "keep\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(destination.join("nested/old.txt"))
            .await
            .unwrap(),
        "old\n"
    );
}

#[tokio::test]
async fn import_directory_merge_rejects_existing_file_destination() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let destination = fixture.workdir.join("release");
    tokio::fs::write(&destination, "not a directory\n")
        .await
        .unwrap();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "merge".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
                (TRANSFER_COMPRESSION_HEADER, "none".to_string()),
            ],
            multi_source_tar(),
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_destination_unsupported");
    assert_eq!(
        tokio::fs::read_to_string(&destination).await.unwrap(),
        "not a directory\n"
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
async fn export_rejects_zstd_when_transfer_compression_is_disabled() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        "enable_transfer_compression = false",
    )
    .await;
    let source = fixture.workdir.join("hello.txt");
    tokio::fs::write(&source, "hello\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
                compression: TransferCompression::Zstd,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_compression_unsupported");
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
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
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
