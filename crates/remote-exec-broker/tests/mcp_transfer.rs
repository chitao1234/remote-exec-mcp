mod support;

use std::io::{Cursor, Read};

use rmcp::model::PaginatedRequestParams;

const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

fn read_single_file_archive(bytes: &[u8]) -> (String, Vec<u8>) {
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut entries = archive.entries().expect("archive entries");
    let mut entry = entries
        .next()
        .expect("archive entry")
        .expect("archive entry ok");
    let path = entry
        .path()
        .expect("entry path")
        .to_string_lossy()
        .into_owned();
    let mut body = Vec::new();
    entry.read_to_end(&mut body).expect("entry body");
    assert!(
        entries
            .next()
            .transpose()
            .expect("no extra entries")
            .is_none(),
        "single-file archive contained extra entries"
    );
    (path, body)
}

fn raw_tar_file_with_path(path: &str, body: &[u8]) -> Vec<u8> {
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

#[cfg(windows)]
fn msys_style_path(path: &std::path::Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    let bytes = text.as_bytes();
    assert!(
        bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic(),
        "expected drive-qualified Windows path, got {text}"
    );

    let drive = (bytes[0] as char).to_ascii_lowercase();
    let rest = text[2..].trim_start_matches('/');
    if rest.is_empty() {
        format!("/{drive}")
    } else {
        format!("/{drive}/{rest}")
    }
}

#[tokio::test]
async fn transfer_files_is_listed_for_mcp_clients() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams {
            meta: None,
            cursor: None,
        }))
        .await
        .expect("list tools");

    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == "transfer_files")
    );
}

#[tokio::test]
async fn transfer_files_copies_local_file_and_reports_summary() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let source = fixture._tempdir.path().join("source.txt");
    let destination = fixture._tempdir.path().join("dest.txt");
    std::fs::write(&source, "hello\n").unwrap();

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": false
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "hello\n");
    assert_eq!(result.structured_content["source_type"], "file");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert_eq!(result.structured_content["directories_copied"], 0);
    assert_eq!(result.structured_content["bytes_copied"], 6);
    assert_eq!(result.structured_content["replaced"], false);
}

#[cfg(windows)]
#[tokio::test]
async fn transfer_files_copies_local_file_using_msys_style_windows_paths() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let source = fixture._tempdir.path().join("source.txt");
    let destination = fixture._tempdir.path().join("dest.txt");
    std::fs::write(&source, "hello\n").unwrap();

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": msys_style_path(&source)
                },
                "destination": {
                    "target": "local",
                    "path": msys_style_path(&destination)
                },
                "overwrite": "fail",
                "create_parent": false
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "hello\n");
    assert_eq!(result.structured_content["source_type"], "file");
    assert_eq!(result.structured_content["files_copied"], 1);
}

#[tokio::test]
async fn transfer_files_copies_local_directory_to_plain_http_remote() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let source = fixture._tempdir.path().join("source");
    std::fs::create_dir_all(source.join("nested/empty")).unwrap();
    std::fs::write(source.join("nested/hello.txt"), "hello remote\n").unwrap();

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "builder-xp",
                    "path": "C:/dest/tree"
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    let capture = fixture
        .last_transfer_import()
        .await
        .expect("transfer import");
    assert_eq!(capture.destination_path, "C:/dest/tree");
    assert_eq!(capture.source_type, "directory");
    assert_eq!(capture.overwrite, "replace");
    assert_eq!(capture.create_parent, "true");
    assert!(capture.body_len > 0);
    assert_eq!(result.structured_content["source_type"], "directory");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert_eq!(result.structured_content["directories_copied"], 3);
    assert_eq!(result.structured_content["replaced"], true);
}

#[tokio::test]
async fn transfer_files_copies_local_file_to_plain_http_remote_as_single_file_tar() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let source = fixture._tempdir.path().join("source.txt");
    std::fs::write(&source, "hello xp\n").unwrap();

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "builder-xp",
                    "path": "C:/dest/file.txt"
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    let capture = fixture
        .last_transfer_import()
        .await
        .expect("transfer import");
    assert_eq!(capture.destination_path, "C:/dest/file.txt");
    assert_eq!(capture.source_type, "file");
    let (path, body) = read_single_file_archive(&capture.body);
    assert_eq!(path, SINGLE_FILE_ENTRY);
    assert_eq!(body, b"hello xp\n");
    assert_eq!(capture.body_len, capture.body.len());
    assert_eq!(result.structured_content["source_type"], "file");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert_eq!(result.structured_content["directories_copied"], 0);
    assert_eq!(result.structured_content["bytes_copied"], 9);
    assert_eq!(result.structured_content["replaced"], false);
}

#[tokio::test]
async fn transfer_files_copies_plain_http_remote_file_to_local_from_single_file_tar() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    fixture
        .set_transfer_export_file_response(b"hello xp\n")
        .await;
    let destination = fixture._tempdir.path().join("dest.txt");

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-xp",
                    "path": "C:/remote/file.txt"
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "hello xp\n");
    assert_eq!(result.structured_content["source_type"], "file");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert_eq!(result.structured_content["directories_copied"], 0);
    assert_eq!(result.structured_content["bytes_copied"], 9);
    assert_eq!(result.structured_content["replaced"], false);
}

#[tokio::test]
async fn transfer_files_copies_plain_http_remote_directory_to_local() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let destination = fixture._tempdir.path().join("dest");

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-xp",
                    "path": "C:/remote-exec/tree"
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(result.structured_content["source_type"], "directory");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert_eq!(result.structured_content["directories_copied"], 3);
    assert_eq!(result.structured_content["replaced"], false);
    assert_eq!(
        std::fs::read_to_string(destination.join("nested/hello.txt")).unwrap(),
        "hello remote\n"
    );
    assert!(destination.join("nested/empty").is_dir());
}

#[tokio::test]
async fn transfer_files_rejects_remote_directory_entries_that_escape_local_destination() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    fixture
        .set_transfer_export_directory_response(raw_tar_file_with_path("../escape.txt", b"owned\n"))
        .await;
    let destination = fixture._tempdir.path().join("dest");
    let escaped = fixture._tempdir.path().join("escape.txt");

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-xp",
                    "path": "C:/remote-exec/tree"
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("must not have `..`") || error.contains("unsupported entry"));
    assert!(!escaped.exists());
}

#[tokio::test]
async fn transfer_files_rejects_same_local_path_before_mutation() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let source = fixture._tempdir.path().join("same.txt");
    std::fs::write(&source, "hello\n").unwrap();

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": false
            }),
        )
        .await;

    assert!(error.contains("source and destination must differ"));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "hello\n");
}

#[tokio::test]
async fn transfer_files_accepts_windows_remote_paths_on_non_windows_hosts() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_platform("windows", false).await;

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": "C:/Work/Artifact.txt"
                },
                "destination": {
                    "target": "builder-a",
                    "path": r"c:\work\artifact.txt"
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("source and destination must differ"));
}

#[tokio::test]
async fn transfer_files_accepts_msys_and_cygwin_windows_remote_paths_on_non_windows_hosts() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_platform("windows", false).await;

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": "/c/Work/Artifact.txt"
                },
                "destination": {
                    "target": "builder-a",
                    "path": "/cygdrive/c/work/artifact.txt"
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("source and destination must differ"));
}

#[cfg(unix)]
#[tokio::test]
async fn transfer_files_still_rejects_windows_paths_for_unix_local_endpoints() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": "C:/Work/Artifact.txt"
                },
                "destination": {
                    "target": "local",
                    "path": "/tmp/out.txt"
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("is not absolute"));
}

#[tokio::test]
async fn transfer_files_applies_host_sandbox_to_local_endpoints() {
    let fixture = support::spawners::spawn_broker_with_local_target_and_host_sandbox_for_workdir(
        |local_workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                local_workdir.join("allowed").display().to_string(),
            )]);
            format!(
                r#"[host_sandbox.read]
allow = {allow}
"#
            )
        },
    )
    .await;
    let blocked_source = fixture.local_workdir().join("blocked/source.txt");
    let allowed_destination = fixture.local_workdir().join("allowed/dest.txt");
    std::fs::create_dir_all(blocked_source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(allowed_destination.parent().unwrap()).unwrap();
    std::fs::write(&blocked_source, "hello\n").unwrap();

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": blocked_source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": allowed_destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": false
            }),
        )
        .await;

    assert!(error.contains("read access"));
    assert_eq!(std::fs::read_to_string(&blocked_source).unwrap(), "hello\n");
    assert!(!allowed_destination.exists());
}
