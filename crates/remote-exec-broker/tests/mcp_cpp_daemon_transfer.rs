#[path = "support/mod.rs"]
mod support;

use remote_exec_broker::ToolResponse;
use support::cpp_daemon::{CPP_TARGET, CppDaemonBrokerFixture};

#[tokio::test]
async fn transfer_files_streams_directory_between_real_cpp_daemon_paths() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn().await else {
        return;
    };

    let source = fixture.daemon_workdir().join("source-tree");
    let destination = fixture.daemon_workdir().join("copied-tree");
    let tool_body = "tool payload\n";
    let log_body = "log payload\n";

    std::fs::create_dir_all(source.join("bin")).unwrap();
    std::fs::create_dir_all(source.join("empty")).unwrap();
    std::fs::create_dir_all(source.join("logs")).unwrap();
    std::fs::write(source.join("bin/tool.txt"), tool_body).unwrap();
    std::fs::write(source.join("logs/output.txt"), log_body).unwrap();

    let result = fixture
        .client
        .call_tool(
            "transfer_files",
            &serde_json::json!({
                "source": {
                    "target": CPP_TARGET,
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": CPP_TARGET,
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await
        .unwrap();

    assert_tool_ok(&result, "transfer_files");
    assert_eq!(
        std::fs::read_to_string(destination.join("bin/tool.txt")).unwrap(),
        tool_body
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("logs/output.txt")).unwrap(),
        log_body
    );
    assert!(destination.join("empty").is_dir());
    assert!(!destination.join("source-tree").exists());

    let expected_bytes = tool_body.len() + log_body.len();
    assert_eq!(result.structured_content["source"]["target"], CPP_TARGET);
    assert_eq!(
        result.structured_content["source"]["path"],
        source.display().to_string()
    );
    assert_eq!(
        result.structured_content["destination"]["target"],
        CPP_TARGET
    );
    assert_eq!(
        result.structured_content["resolved_destination"]["target"],
        CPP_TARGET
    );
    assert_eq!(
        result.structured_content["resolved_destination"]["path"],
        destination.display().to_string()
    );
    assert_eq!(result.structured_content["destination_mode"], "auto");
    assert_eq!(result.structured_content["symlink_mode"], "preserve");
    assert_eq!(result.structured_content["source_type"], "directory");
    assert_eq!(result.structured_content["files_copied"], 2);
    assert_eq!(result.structured_content["directories_copied"], 4);
    assert_eq!(result.structured_content["bytes_copied"], expected_bytes);
    assert_eq!(result.structured_content["replaced"], false);
    assert_eq!(
        result.structured_content["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        result.text_output,
        format!(
            "Transfer complete.\nSource: directory `{}` on `{}`\nDestination: `{}` on `{}`\nFiles copied: 2\nDirectories copied: 4\nBytes copied: {expected_bytes}\nReplaced existing destination: no",
            source.display(),
            CPP_TARGET,
            destination.display(),
            CPP_TARGET
        )
    );
}

fn assert_tool_ok(result: &ToolResponse, tool: &str) {
    assert!(!result.is_error, "{tool} failed: {}", result.text_output);
}
