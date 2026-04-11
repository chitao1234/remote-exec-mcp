use super::*;

#[tokio::test]
async fn broker_keeps_healthy_targets_available_when_one_target_is_down() {
    let fixture = support::spawners::spawn_broker_with_live_and_dead_targets().await;

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
    let fixture = support::spawners::spawn_broker_with_late_target().await;
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
async fn list_targets_clears_cached_daemon_info_after_daemon_instance_mismatch() {
    let fixture = support::spawners::spawn_broker_with_retryable_exec_write_error().await;

    let before = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert!(before.structured_content["targets"][0]["daemon_info"].is_object());

    let session_id = fixture.start_running_session().await;
    fixture
        .set_stub_daemon_instance_id("daemon-instance-2")
        .await;

    let error = fixture
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 10
            }),
        )
        .await;
    assert!(error.contains("Unknown process id"));

    let after = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert!(after.structured_content["targets"][0]["daemon_info"].is_null());
}

#[tokio::test]
async fn list_targets_repopulates_cached_daemon_info_after_later_successful_verification() {
    let fixture = support::spawners::spawn_broker_with_late_target().await;

    let before = fixture
        .broker
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert!(before.structured_content["targets"][1]["daemon_info"].is_null());

    fixture.spawn_target("builder-b").await;
    let result = fixture
        .broker
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-b",
                "input": "*** Begin Patch\n*** Add File: ok.txt\n+ok\n*** End Patch\n"
            }),
        )
        .await;
    assert!(result.text_output.contains("Success."));

    let after = fixture
        .broker
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert_eq!(
        after.structured_content["targets"][1]["daemon_info"]["hostname"],
        "builder-b-host"
    );
}
