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
