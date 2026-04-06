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
