use super::*;

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
