mod support;

use encoding_rs::SHIFT_JIS;
use remote_exec_proto::rpc::{
    FileEditRequest, FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};
use remote_exec_test_support::test_helpers::{DEFAULT_TEST_TARGET, utf16le_bom_bytes};
use support::encoded_bytes;

#[tokio::test]
async fn read_file_formats_line_numbers_and_reminders() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    tokio::fs::write(fixture.workdir.join("sample.txt"), "one\ntwo\nthree\n")
        .await
        .unwrap();

    let response = fixture
        .rpc::<_, FileReadResponse>(
            "/v1/file/read",
            &FileReadRequest {
                path: "sample.txt".to_string(),
                offset: Some(0),
                limit: 2,
                max_bytes: 1024,
            },
        )
        .await;

    assert_eq!(
        response.output,
        "1: one\n2: two\n\n(showing lines 1-2 of 3 lines)"
    );
    assert_eq!(response.lines_returned, 2);
    assert_eq!(response.total_lines, 3);
    assert!(!response.eof);
}

#[tokio::test]
async fn write_and_edit_file_update_default_workdir_paths() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;

    let write = fixture
        .rpc::<_, FileWriteResponse>(
            "/v1/file/write",
            &FileWriteRequest {
                path: "demo.txt".to_string(),
                content: "red blue red\n".to_string(),
                max_bytes: 1024,
            },
        )
        .await;
    assert!(write.created);
    assert_eq!(write.line_count, 1);

    let err = fixture
        .rpc_error(
            "/v1/file/edit",
            &FileEditRequest {
                path: "demo.txt".to_string(),
                old_string: "red".to_string(),
                new_string: "green".to_string(),
                replace_all: false,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(err.wire_code(), "file_old_string_ambiguous");

    let edit = fixture
        .rpc::<_, FileEditResponse>(
            "/v1/file/edit",
            &FileEditRequest {
                path: "demo.txt".to_string(),
                old_string: "red".to_string(),
                new_string: "green".to_string(),
                replace_all: true,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(edit.replacements, 2);
    assert_eq!(edit.line_count, 1);
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("demo.txt"))
            .await
            .unwrap(),
        "green blue green\n"
    );
}

#[tokio::test]
async fn read_write_and_edit_enforce_existing_file_size_limit() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    tokio::fs::write(fixture.workdir.join("large.txt"), "0123456789\n")
        .await
        .unwrap();

    let read = fixture
        .rpc_error(
            "/v1/file/read",
            &FileReadRequest {
                path: "large.txt".to_string(),
                offset: None,
                limit: 10,
                max_bytes: 4,
            },
        )
        .await;
    assert_eq!(read.wire_code(), "file_too_large");

    let write = fixture
        .rpc_error(
            "/v1/file/write",
            &FileWriteRequest {
                path: "large.txt".to_string(),
                content: "small\n".to_string(),
                max_bytes: 4,
            },
        )
        .await;
    assert_eq!(write.wire_code(), "file_too_large");

    let edit = fixture
        .rpc_error(
            "/v1/file/edit",
            &FileEditRequest {
                path: "large.txt".to_string(),
                old_string: "0".to_string(),
                new_string: "x".to_string(),
                replace_all: false,
                max_bytes: 4,
            },
        )
        .await;
    assert_eq!(edit.wire_code(), "file_too_large");
}

#[tokio::test]
async fn file_tools_enforce_sandbox_access() {
    let fixture = support::spawn::spawn_daemon_with_extra_config_for_workdir(
        DEFAULT_TEST_TARGET,
        |workdir| {
            let read_allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("readable").display().to_string(),
            )]);
            let write_allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("writable").display().to_string(),
            )]);
            format!(
                r#"[sandbox.read]
allow = {read_allow}

[sandbox.write]
allow = {write_allow}
"#
            )
        },
    )
    .await;
    tokio::fs::write(fixture.workdir.join("blocked-read.txt"), "blocked\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("blocked-write.txt"), "blocked\n")
        .await
        .unwrap();

    let read = fixture
        .rpc_error(
            "/v1/file/read",
            &FileReadRequest {
                path: "blocked-read.txt".to_string(),
                offset: None,
                limit: 10,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(read.wire_code(), "sandbox_denied");
    assert!(read.message.contains("read access"));

    let write = fixture
        .rpc_error(
            "/v1/file/write",
            &FileWriteRequest {
                path: "blocked-write.txt".to_string(),
                content: "updated\n".to_string(),
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(write.wire_code(), "sandbox_denied");
    assert!(write.message.contains("write access"));

    let edit = fixture
        .rpc_error(
            "/v1/file/edit",
            &FileEditRequest {
                path: "blocked-write.txt".to_string(),
                old_string: "blocked".to_string(),
                new_string: "updated".to_string(),
                replace_all: false,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(edit.wire_code(), "sandbox_denied");
    assert!(edit.message.contains("write access"));
}

#[tokio::test]
async fn edit_rejects_empty_old_string() {
    let fixture = support::spawn::spawn_daemon(DEFAULT_TEST_TARGET).await;
    tokio::fs::write(fixture.workdir.join("demo.txt"), "hello\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/file/edit",
            &FileEditRequest {
                path: "demo.txt".to_string(),
                old_string: String::new(),
                new_string: "updated".to_string(),
                replace_all: false,
                max_bytes: 1024,
            },
        )
        .await;

    assert_eq!(err.wire_code(), "bad_request");
    assert!(err.message.contains("old_string"));
}

#[tokio::test]
async fn read_write_and_edit_preserve_autodetected_utf16le_encoding() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        DEFAULT_TEST_TARGET,
        support::ENCODING_AUTODETECT_CONFIG,
    )
    .await;
    let path = fixture.workdir.join("utf16.txt");
    tokio::fs::write(&path, utf16le_bom_bytes("hello\nworld\n"))
        .await
        .unwrap();

    let read = fixture
        .rpc::<_, FileReadResponse>(
            "/v1/file/read",
            &FileReadRequest {
                path: "utf16.txt".to_string(),
                offset: None,
                limit: 10,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(
        read.output,
        "1: hello\n2: world\n\n(EOF reached, file has 2 lines)"
    );

    fixture
        .rpc::<_, FileEditResponse>(
            "/v1/file/edit",
            &FileEditRequest {
                path: "utf16.txt".to_string(),
                old_string: "world".to_string(),
                new_string: "there".to_string(),
                replace_all: false,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(
        tokio::fs::read(&path).await.unwrap(),
        utf16le_bom_bytes("hello\nthere\n")
    );

    fixture
        .rpc::<_, FileWriteResponse>(
            "/v1/file/write",
            &FileWriteRequest {
                path: "utf16.txt".to_string(),
                content: "done\n".to_string(),
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(
        tokio::fs::read(&path).await.unwrap(),
        utf16le_bom_bytes("done\n")
    );
}

#[tokio::test]
async fn write_and_edit_report_write_failure_for_unencodable_detected_text() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        DEFAULT_TEST_TARGET,
        support::ENCODING_AUTODETECT_CONFIG,
    )
    .await;
    let path = fixture.workdir.join("shift-jis.txt");
    tokio::fs::write(&path, encoded_bytes(SHIFT_JIS, "価格\n"))
        .await
        .unwrap();

    let write = fixture
        .rpc_error(
            "/v1/file/write",
            &FileWriteRequest {
                path: "shift-jis.txt".to_string(),
                content: "価格 😀\n".to_string(),
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(write.wire_code(), "file_write_failed");
    assert!(write.message.contains("unable to encode updated text"));

    let edit = fixture
        .rpc_error(
            "/v1/file/edit",
            &FileEditRequest {
                path: "shift-jis.txt".to_string(),
                old_string: "価格".to_string(),
                new_string: "価格 😀".to_string(),
                replace_all: false,
                max_bytes: 1024,
            },
        )
        .await;
    assert_eq!(edit.wire_code(), "file_write_failed");
    assert!(edit.message.contains("unable to encode updated text"));
}
