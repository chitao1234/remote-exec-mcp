use super::*;

#[cfg(unix)]
const LOCAL_TEST_SHELL: &str = "/bin/sh";
#[cfg(windows)]
const LOCAL_TEST_SHELL: &str = "cmd.exe";

#[cfg(unix)]
const LOCAL_LONG_RUNNING_CMD: &str = "printf ready; sleep 2";
#[cfg(windows)]
const LOCAL_LONG_RUNNING_CMD: &str = "echo ready & ping -n 3 127.0.0.1 >nul";

#[tokio::test]
async fn exec_command_and_write_stdin_work_for_enabled_local_target() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;
    let started = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "local",
                "cmd": LOCAL_LONG_RUNNING_CMD,
                "shell": LOCAL_TEST_SHELL,
                "tty": false,
                "yield_time_ms": 250
            }),
        )
        .await;

    let session_id = started.structured_content["session_id"]
        .as_str()
        .expect("running local session")
        .to_string();
    assert_eq!(started.structured_content["target"], "local");
    assert_eq!(
        started.structured_content["session_command"],
        serde_json::Value::String(LOCAL_LONG_RUNNING_CMD.to_string())
    );

    let polled = fixture
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 5_000
            }),
        )
        .await;

    assert_eq!(polled.structured_content["target"], "local");
    assert!(polled.text_output.contains("Command:"));
    assert_eq!(
        polled.structured_content["session_command"],
        serde_json::Value::String(LOCAL_LONG_RUNNING_CMD.to_string())
    );
}
