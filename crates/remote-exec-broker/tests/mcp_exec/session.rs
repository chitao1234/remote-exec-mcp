use super::*;

#[tokio::test]
async fn write_stdin_routes_by_public_session_id_and_preserves_original_command_metadata() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let started = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;
    let session_id = started.structured_content["session_id"]
        .as_str()
        .expect("running session")
        .to_string();

    let result = fixture
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;

    assert_eq!(result.structured_content["target"], "builder-a");
    assert!(
        result.structured_content["output"]
            .as_str()
            .unwrap()
            .contains("poll output")
    );

    assert!(
        result
            .text_output
            .contains("Command: printf ready; sleep 2")
    );
    assert_eq!(
        result.structured_content["session_command"],
        serde_json::Value::String("printf ready; sleep 2".to_string())
    );
}

#[tokio::test]
async fn write_stdin_forwards_pty_size_to_daemon_session() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let start = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "sleep 30",
                "tty": true,
                "yield_time_ms": 250,
            }),
        )
        .await;
    let session_id = start
        .structured_content
        .get("session_id")
        .and_then(|value| value.as_str())
        .expect("public session id");

    fixture
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "pty_size": {
                    "rows": 33,
                    "cols": 101,
                },
            }),
        )
        .await;

    let forwarded = fixture
        .last_exec_write_request()
        .await
        .expect("write request");
    assert_eq!(
        forwarded.pty_size,
        Some(remote_exec_proto::rpc::ExecPtySize {
            rows: 33,
            cols: 101
        })
    );
}

#[tokio::test]
async fn write_stdin_wraps_unknown_public_session_as_unknown_process_id() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": "sess_missing",
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;

    assert_eq!(error, "write_stdin failed: Unknown process id sess_missing");
}

#[tokio::test]
async fn write_stdin_wraps_daemon_unknown_session_as_unknown_process_id() {
    let fixture = support::spawners::spawn_broker_with_unknown_session_exec_write_error().await;

    let started = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 30",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;
    let session_id = started.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let error = fixture
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;

    assert_eq!(
        error,
        format!(
            "write_stdin failed: Unknown process id {}",
            started.structured_content["session_id"].as_str().unwrap()
        )
    );
}

#[tokio::test]
async fn write_stdin_keeps_session_after_retryable_daemon_error() {
    let fixture = support::spawners::spawn_broker_with_retryable_exec_write_error().await;

    let started = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 30",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;
    let session_id = started.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let first = fixture
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 250
            }),
        )
        .await;
    assert!(first.starts_with("write_stdin failed: "));
    assert!(first.contains("temporary_failure"));

    let second = fixture
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": started.structured_content["session_id"],
                "chars": "",
                "yield_time_ms": 250
            }),
        )
        .await;
    assert_eq!(second.structured_content["target"], "builder-a");
}
