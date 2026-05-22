mod support;

use remote_exec_proto::rpc::{
    FileEditRequest, FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};
use remote_exec_test_support::test_helpers::{DEFAULT_TEST_TARGET, utf16le_bom_bytes};

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
async fn read_write_and_edit_preserve_autodetected_utf16le_encoding() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        DEFAULT_TEST_TARGET,
        "experimental_apply_patch_target_encoding_autodetect = true",
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
            },
        )
        .await;
    assert_eq!(
        tokio::fs::read(&path).await.unwrap(),
        utf16le_bom_bytes("done\n")
    );
}
