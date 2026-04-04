mod support;

#[tokio::test]
async fn exec_command_returns_opaque_session_id_and_session_command() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
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
    assert_eq!(
        result.structured_content["session_command"],
        serde_json::Value::String("printf ready; sleep 2".to_string())
    );
}

#[tokio::test]
async fn exec_command_intercepts_direct_apply_patch_and_wraps_exec_output() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: hello.txt\n",
        "+hello\n",
        "*** End Patch\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("apply_patch '{patch}'"),
            }),
        )
        .await;

    assert!(result.text_output.contains("Wall time: 0.0000 seconds"));
    assert!(result.text_output.contains("Process exited with code 0"));
    assert!(
        result
            .text_output
            .contains("Output:\nSuccess. Updated the following files:")
    );
    assert!(!result.text_output.contains("Command:"));
    assert!(!result.text_output.contains("Chunk ID:"));
    assert!(result.structured_content["session_id"].is_null());
    assert!(result.structured_content["session_command"].is_null());
    assert_eq!(result.structured_content["wall_time_seconds"], 0.0);
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        patch.to_string()
    );
}

#[tokio::test]
async fn exec_command_non_matching_patch_text_still_uses_exec_start() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let raw_patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: raw.txt\n",
        "+raw\n",
        "*** End Patch\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": raw_patch,
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    assert!(result.text_output.contains("Command: *** Begin Patch"));
    assert!(result.structured_content["session_id"].as_str().is_some());
    assert_eq!(fixture.exec_start_calls().await, 1);
    assert!(fixture.last_patch_request().await.is_none());
}

#[tokio::test]
async fn exec_command_intercepts_applypatch_heredoc_with_cd_wrapper() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: hello.txt\n",
        "+hello\n",
        "*** End Patch\n",
    );
    let cmd = concat!(
        "cd nested && applypatch <<'PATCH'\n",
        "*** Begin Patch\n",
        "*** Add File: hello.txt\n",
        "+hello\n",
        "*** End Patch\n",
        "PATCH\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": cmd,
                "workdir": "outer"
            }),
        )
        .await;

    assert!(
        result
            .text_output
            .contains("Output:\nSuccess. Updated the following files:")
    );
    assert_eq!(fixture.exec_start_calls().await, 0);
    let forwarded = fixture.last_patch_request().await.unwrap();
    assert_eq!(forwarded.patch, patch.to_string());
    assert_eq!(forwarded.workdir, Some("outer/nested".to_string()));
}

#[tokio::test]
async fn exec_command_intercepts_apply_patch_whitespace_tolerant_forms() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let direct_patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: direct.txt\n",
        "+direct\n",
        "*** End Patch\n",
    );

    let direct = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!(" \tapply_patch\t  '{direct_patch}' \t"),
            }),
        )
        .await;

    assert!(direct.text_output.contains("Process exited with code 0"));
    assert!(
        direct
            .text_output
            .contains("Output:\nSuccess. Updated the following files:")
    );
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        direct_patch.to_string()
    );

    let heredoc_fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let heredoc_cmd = concat!(
        "cd\t nested  && \tapplypatch\t <<'PATCH'\n",
        "*** Begin Patch\n",
        "*** Add File: heredoc.txt\n",
        "+heredoc\n",
        "*** End Patch\n",
        "PATCH\n",
    );

    let heredoc = heredoc_fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": heredoc_cmd,
                "workdir": "outer"
            }),
        )
        .await;

    assert!(
        heredoc
            .text_output
            .contains("Output:\nSuccess. Updated the following files:")
    );
    assert_eq!(heredoc_fixture.exec_start_calls().await, 0);
    let forwarded = heredoc_fixture.last_patch_request().await.unwrap();
    assert_eq!(forwarded.workdir, Some("outer/nested".to_string()));
    assert_eq!(
        forwarded.patch,
        concat!(
            "*** Begin Patch\n",
            "*** Add File: heredoc.txt\n",
            "+heredoc\n",
            "*** End Patch\n",
        )
        .to_string()
    );
}

#[tokio::test]
async fn exec_command_intercepts_windows_shell_wrappers() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_platform("windows", false).await;
    let patch = "*** Begin Patch\n*** Add File: wrapped.txt\n+wrapped\n*** End Patch\n";

    let cmd_result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("cmd /c apply_patch '{patch}'"),
            }),
        )
        .await;
    assert!(
        cmd_result
            .text_output
            .contains("Process exited with code 0")
    );

    let pwsh_result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("pwsh -NoProfile -Command \"apply_patch '{patch}'\""),
            }),
        )
        .await;
    assert!(
        pwsh_result
            .text_output
            .contains("Process exited with code 0")
    );
}

#[tokio::test]
async fn exec_command_invalid_intercepted_patch_surfaces_tool_error() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "apply_patch 'not a patch'"
            }),
        )
        .await;

    assert!(error.contains("patch_failed") || error.contains("invalid patch"));
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        "not a patch".to_string()
    );
}

#[tokio::test]
async fn exec_command_intercepted_apply_patch_warning_success_in_meta() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let patch = "*** Begin Patch\n*** Add File: warning.txt\n+warning\n*** End Patch\n";

    let result = fixture
        .raw_tool_result(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("apply_patch '{patch}'"),
            }),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(
        result.meta.as_ref().unwrap()["warnings"][0]["code"],
        "apply_patch_via_exec_command"
    );
}

#[tokio::test]
async fn exec_command_intercepted_apply_patch_warning_error_in_meta() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .raw_tool_result(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "apply_patch 'not a patch'",
            }),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(
        result.meta.as_ref().unwrap()["warnings"][0]["message"],
        "Use apply_patch directly rather than through exec_command."
    );
}

#[tokio::test]
async fn exec_command_forwarded_session_warning_in_meta() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    fixture
        .set_exec_start_warnings(vec![remote_exec_proto::rpc::ExecWarning {
            code: "exec_session_limit_approaching".to_string(),
            message: "Target `builder-a` now has 60 open exec sessions.".to_string(),
        }])
        .await;

    let result = fixture
        .raw_tool_result(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(
        result.meta.as_ref().unwrap()["warnings"][0]["code"],
        "exec_session_limit_approaching"
    );
}

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
