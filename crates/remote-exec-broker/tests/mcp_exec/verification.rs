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

#[tokio::test]
async fn broker_can_skip_server_name_verification_for_https_targets() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_and_daemon_spec(
        remote_exec_pki::DaemonCertSpec {
            target: "builder-a".to_string(),
            sans: vec![remote_exec_pki::SubjectAltName::Dns(
                "builder-a.example.com".to_string(),
            )],
        },
        "skip_server_name_verification = true",
    )
    .await;

    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: skip-san.txt\n+ok\n*** End Patch\n"
            }),
        )
        .await;

    assert!(result.text_output.contains("Success."));
}

#[tokio::test]
async fn broker_accepts_matching_pinned_server_certificate() {
    let tempdir = tempfile::tempdir().unwrap();
    let certs = support::certs::write_test_certs(tempdir.path());
    let extra_target_config = format!(
        "pinned_server_cert_pem = {}",
        toml::Value::String(certs.daemon_cert.display().to_string())
    );
    let fixture = support::spawners::spawn_broker_with_stub_daemon_and_extra_target_config(
        certs.clone(),
        &extra_target_config,
    )
    .await;

    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: pinned.txt\n+ok\n*** End Patch\n"
            }),
        )
        .await;

    assert!(result.text_output.contains("Success."));
}

#[tokio::test]
async fn broker_rejects_mismatched_pinned_server_certificate() {
    let tempdir = tempfile::tempdir().unwrap();
    let certs = support::certs::write_test_certs(tempdir.path());
    let wrong_dir = tempdir.path().join("wrong-pin");
    std::fs::create_dir_all(&wrong_dir).unwrap();
    let wrong_pin = support::certs::write_test_certs_for_daemon_spec(
        &wrong_dir,
        remote_exec_pki::DaemonCertSpec {
            target: "builder-b".to_string(),
            sans: vec![remote_exec_pki::SubjectAltName::Dns(
                "builder-b.example.com".to_string(),
            )],
        },
    );
    let extra_target_config = format!(
        "pinned_server_cert_pem = {}",
        toml::Value::String(wrong_pin.daemon_cert.display().to_string())
    );
    let fixture = support::spawners::spawn_broker_with_stub_daemon_and_extra_target_config(
        certs,
        &extra_target_config,
    )
    .await;

    let error = fixture
        .call_tool_error(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: bad-pin.txt\n+nope\n*** End Patch\n"
            }),
        )
        .await;

    assert!(error.contains("pinned server certificate mismatch"));
}
