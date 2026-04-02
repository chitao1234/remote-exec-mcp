mod support;

use rmcp::model::PaginatedRequestParams;

#[tokio::test]
async fn transfer_files_is_listed_for_mcp_clients() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
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
    let fixture = support::spawn_broker_with_stub_daemon().await;
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

#[tokio::test]
async fn transfer_files_rejects_same_local_path_before_mutation() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
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
