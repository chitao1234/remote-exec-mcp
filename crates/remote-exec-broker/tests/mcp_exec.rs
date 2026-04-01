mod support;

#[tokio::test]
async fn exec_command_returns_an_opaque_string_session_id() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let result = fixture
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

    let session_id = result.structured_content["session_id"]
        .as_str()
        .expect("running session");
    assert!(session_id.starts_with("sess_"));
    assert!(result.structured_content["exit_code"].is_null());
}

#[tokio::test]
async fn exec_command_structured_output_includes_session_command() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let result = fixture
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

    assert_eq!(
        result.structured_content["session_command"],
        serde_json::Value::String("printf ready; sleep 2".to_string())
    );
}

#[tokio::test]
async fn write_stdin_routes_by_public_session_id_instead_of_target_guessing() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
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
}

#[tokio::test]
async fn broker_keeps_healthy_targets_available_when_one_target_is_down() {
    let fixture = support::spawn_broker_with_live_and_dead_targets().await;

    let healthy = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: ok.txt\n+ok\n*** End Patch\n"
            }),
        )
        .await;
    assert!(healthy.text_output.contains("Success."));

    let dead = fixture
        .call_tool_error(
            "exec_command",
            serde_json::json!({
                "target": "builder-b",
                "cmd": "pwd"
            }),
        )
        .await;
    assert!(dead.contains("daemon") || dead.contains("connection"));
}

#[tokio::test]
async fn broker_rejects_unverified_target_if_it_returns_as_the_wrong_daemon() {
    let fixture = support::spawn_broker_with_late_target().await;
    fixture
        .broker
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: ok.txt\n+ok\n*** End Patch\n"
            }),
        )
        .await;

    fixture.spawn_target("not-builder-b").await;

    let wrong = fixture
        .broker
        .call_tool_error(
            "apply_patch",
            serde_json::json!({
                "target": "builder-b",
                "input": "*** Begin Patch\n*** Add File: nope.txt\n+nope\n*** End Patch\n"
            }),
        )
        .await;

    assert!(wrong.contains("resolved to daemon `not-builder-b` instead of `builder-b`"));
}

#[tokio::test]
async fn write_stdin_keeps_session_after_retryable_daemon_error() {
    let fixture = support::spawn_broker_with_retryable_exec_write_error().await;

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
