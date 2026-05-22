mod support;

use remote_exec_broker::{Connection, RemoteExecClient};
use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;
use rmcp::model::PaginatedRequestParams;

fn toml_path(path: &std::path::Path) -> String {
    toml::Value::String(path.display().to_string()).to_string()
}

#[tokio::test]
async fn hidden_file_tools_are_not_listed_by_default() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams::default()))
        .await
        .expect("list tools");
    let names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!names.contains("read"));
    assert!(!names.contains("write"));
    assert!(!names.contains("edit"));
}

#[tokio::test]
async fn hidden_file_tools_are_listed_when_enabled() {
    let fixture = support::spawners::spawn_broker_with_local_target_and_extra_config(
        hidden_file_tools_config(),
    )
    .await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams::default()))
        .await
        .expect("list tools");
    let names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<std::collections::BTreeSet<_>>();

    assert!(names.contains("read"));
    assert!(names.contains("write"));
    assert!(names.contains("edit"));

    let read = tools
        .tools
        .into_iter()
        .find(|tool| tool.name.as_ref() == "read")
        .expect("read tool");
    assert_eq!(
        read.annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
        Some(true)
    );
}

#[tokio::test]
async fn direct_client_rejects_hidden_file_tools_when_disabled() {
    let tempdir = tempfile::tempdir().unwrap();
    let workdir = tempdir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let config_path = tempdir.path().join("broker.toml");
    std::fs::write(
        &config_path,
        format!("[local]\ndefault_workdir = {}\n", toml_path(&workdir)),
    )
    .unwrap();
    let client = RemoteExecClient::connect(Connection::Config { config_path })
        .await
        .unwrap();

    let result = client
        .call_tool(
            "read",
            &serde_json::json!({
                "target": "local",
                "file_path": "missing.txt"
            }),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(result.text_output, "unknown tool `read`");
}

#[tokio::test]
async fn enabled_file_read_formats_line_numbers_and_reminders() {
    let fixture = support::spawners::spawn_broker_with_local_target_and_extra_config(
        hidden_file_tools_config(),
    )
    .await;
    let file_path = fixture.local_workdir().join("sample.txt");
    std::fs::write(&file_path, "one\ntwo\nthree\n").unwrap();

    let result = fixture
        .call_tool(
            "read",
            serde_json::json!({
                "target": "local",
                "file_path": "sample.txt",
                "offset": 0,
                "limit": 2
            }),
        )
        .await;

    assert_eq!(
        result.text_output,
        "1: one\n2: two\n\n(showing lines 1-2 of 3 lines)"
    );
    assert_eq!(result.structured_content, serde_json::Value::Null);

    let result = fixture
        .call_tool(
            "read",
            serde_json::json!({
                "target": "local",
                "file_path": "sample.txt",
                "offset": 99,
                "limit": 2
            }),
        )
        .await;

    assert_eq!(
        result.text_output,
        "(offset out of range, file only has 3 lines)"
    );
}

#[tokio::test]
async fn remote_file_tools_forward_configured_byte_limit() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_and_extra_config(
        hidden_file_tools_config(),
    )
    .await;

    fixture
        .call_tool(
            "read",
            serde_json::json!({
                "target": DEFAULT_TEST_TARGET,
                "file_path": "sample.txt"
            }),
        )
        .await;
    assert_eq!(
        fixture.last_file_read_request().await.unwrap().max_bytes,
        1048576
    );

    fixture
        .call_tool(
            "write",
            serde_json::json!({
                "target": DEFAULT_TEST_TARGET,
                "file_path": "sample.txt",
                "content": "updated\n"
            }),
        )
        .await;
    assert_eq!(
        fixture.last_file_write_request().await.unwrap().max_bytes,
        1048576
    );

    fixture
        .call_tool(
            "edit",
            serde_json::json!({
                "target": DEFAULT_TEST_TARGET,
                "file_path": "sample.txt",
                "old_string": "before",
                "new_string": "after"
            }),
        )
        .await;
    assert_eq!(
        fixture.last_file_edit_request().await.unwrap().max_bytes,
        1048576
    );
}

#[tokio::test]
async fn remote_file_tools_reject_targets_without_file_capability() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_without_file_tool_support(
        hidden_file_tools_config(),
    )
    .await;

    let err = fixture
        .call_tool_error(
            "read",
            serde_json::json!({
                "target": DEFAULT_TEST_TARGET,
                "file_path": "sample.txt"
            }),
        )
        .await;

    assert!(err.contains("does not support file tool protocol version 1"));
    assert!(fixture.last_file_read_request().await.is_none());
}

#[tokio::test]
async fn enabled_file_write_creates_and_updates_files() {
    let fixture = support::spawners::spawn_broker_with_local_target_and_extra_config(
        hidden_file_tools_config(),
    )
    .await;

    let result = fixture
        .call_tool(
            "write",
            serde_json::json!({
                "target": "local",
                "file_path": "created.txt",
                "content": "alpha\nbeta\n"
            }),
        )
        .await;

    assert_eq!(result.text_output, "file created successfully with 2 lines");
    assert_eq!(
        std::fs::read_to_string(fixture.local_workdir().join("created.txt")).unwrap(),
        "alpha\nbeta\n"
    );

    let result = fixture
        .call_tool(
            "write",
            serde_json::json!({
                "target": "local",
                "file_path": "created.txt",
                "content": "gamma\n"
            }),
        )
        .await;

    assert_eq!(result.text_output, "file updated successfully with 1 lines");
    assert_eq!(
        std::fs::read_to_string(fixture.local_workdir().join("created.txt")).unwrap(),
        "gamma\n"
    );
}

#[tokio::test]
async fn enabled_file_edit_replaces_unique_or_all_matches() {
    let fixture = support::spawners::spawn_broker_with_local_target_and_extra_config(
        hidden_file_tools_config(),
    )
    .await;
    let file_path = fixture.local_workdir().join("edit.txt");
    std::fs::write(&file_path, "red blue red\n").unwrap();

    let err = fixture
        .call_tool_error(
            "edit",
            serde_json::json!({
                "target": "local",
                "file_path": "edit.txt",
                "old_string": "red",
                "new_string": "green"
            }),
        )
        .await;
    assert!(
        err.contains("file_old_string_ambiguous") || err.contains("old_string matched 2 times"),
        "{err}"
    );

    let result = fixture
        .call_tool(
            "edit",
            serde_json::json!({
                "target": "local",
                "file_path": "edit.txt",
                "old_string": "red",
                "new_string": "green",
                "replace_all": true
            }),
        )
        .await;

    assert_eq!(
        result.text_output,
        "file updated successfully with 1 lines, 2 replacements"
    );
    assert_eq!(
        std::fs::read_to_string(file_path).unwrap(),
        "green blue green\n"
    );
}

fn hidden_file_tools_config() -> &'static str {
    r#"[tools.file]
read = true
write = true
edit = true
default_read_limit_lines = 2
max_read_limit_lines = 20
max_read_bytes = 1048576
"#
}
