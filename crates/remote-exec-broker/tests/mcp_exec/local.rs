use super::*;

#[cfg(unix)]
const LOCAL_TEST_SHELL: &str = "/bin/sh";
#[cfg(windows)]
const LOCAL_TEST_SHELL: &str = "cmd.exe";

#[cfg(unix)]
const LOCAL_LONG_RUNNING_CMD: &str = "printf ready; sleep 2";
#[cfg(windows)]
const LOCAL_LONG_RUNNING_CMD: &str = "echo ready & ping -n 3 127.0.0.1 >nul";

#[cfg(windows)]
const LOCAL_PIPE_MODE_STDOUT_STDERR_CMD: &str =
    "echo stdout-1&echo stderr-1>&2&echo stdout-2&echo stderr-2>&2";

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
                "login": false,
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

#[tokio::test]
async fn exec_command_uses_local_yield_time_policy_overrides() {
    let fixture = support::spawners::spawn_broker_with_local_target_and_extra_config(
        r#"[local.yield_time.exec_command]
default_ms = 3000
max_ms = 3000
min_ms = 3000
"#,
    )
    .await;
    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "local",
                "cmd": LOCAL_LONG_RUNNING_CMD,
                "shell": LOCAL_TEST_SHELL,
                "login": false,
                "tty": false,
                "yield_time_ms": 1
            }),
        )
        .await;

    assert!(result.structured_content["session_id"].is_null());
    assert_eq!(result.structured_content["exit_code"], 0);
    assert!(result.text_output.contains("ready"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_command_local_pipe_mode_preserves_stdout_stderr_order() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;
    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "local",
                "cmd": "printf 'stdout-1\\n'; printf 'stderr-1\\n' >&2; printf 'stdout-2\\n'; printf 'stderr-2\\n' >&2",
                "shell": LOCAL_TEST_SHELL,
                "login": false,
                "tty": false,
                "yield_time_ms": 10_000
            }),
        )
        .await;

    assert_eq!(result.structured_content["exit_code"], 0);
    assert_eq!(
        result.structured_content["output"],
        serde_json::json!("stdout-1\nstderr-1\nstdout-2\nstderr-2\n")
    );
}

#[cfg(windows)]
#[tokio::test]
async fn exec_command_local_pipe_mode_preserves_stdout_stderr_order() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;
    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "local",
                "cmd": LOCAL_PIPE_MODE_STDOUT_STDERR_CMD,
                "shell": LOCAL_TEST_SHELL,
                "login": false,
                "tty": false,
                "yield_time_ms": 10_000
            }),
        )
        .await;

    assert_eq!(result.structured_content["exit_code"], 0);
    assert_eq!(
        result.structured_content["output"]
            .as_str()
            .unwrap()
            .replace("\r\n", "\n"),
        "stdout-1\nstderr-1\nstdout-2\nstderr-2\n"
    );
}
