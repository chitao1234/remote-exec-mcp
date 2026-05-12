use super::*;

#[tokio::test]
async fn exec_command_reports_malformed_running_response_without_daemon_session_id() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    fixture.set_malformed_exec_start_missing_session_id().await;

    let error = fixture
        .call_tool_error(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    support::assert_correlated_tool_error(
        &error,
        "exec_command",
        Some("builder-a"),
        "daemon returned malformed exec response: running response missing daemon_session_id",
    );
}

#[tokio::test]
async fn write_stdin_reports_malformed_completed_response_without_exit_code() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let session_id = fixture.start_running_session().await;
    fixture.set_malformed_exec_write_missing_exit_code().await;

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

    support::assert_correlated_tool_error(
        &error,
        "write_stdin",
        Some("builder-a"),
        "write_stdin failed: daemon returned malformed exec response: completed response missing exit_code",
    );
}
