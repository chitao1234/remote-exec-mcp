#![cfg(all(feature = "broker-tls", feature = "daemon-tls"))]

#[path = "support/mod.rs"]
mod support;

use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;

#[tokio::test]
async fn broker_can_skip_server_name_verification_for_https_targets() {
    let fixture = support::spawners::spawn_broker_with_tls_stub_daemon_and_daemon_spec(
        remote_exec_pki::DaemonCertSpec {
            target: DEFAULT_TEST_TARGET.to_string(),
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
                "target": DEFAULT_TEST_TARGET,
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
    let fixture = support::spawners::spawn_broker_with_tls_stub_daemon_and_extra_target_config(
        certs.clone(),
        &extra_target_config,
    )
    .await;

    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": DEFAULT_TEST_TARGET,
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
    let fixture = support::spawners::spawn_broker_with_tls_stub_daemon_and_extra_target_config(
        certs,
        &extra_target_config,
    )
    .await;

    let error = fixture
        .call_tool_error(
            "apply_patch",
            serde_json::json!({
                "target": DEFAULT_TEST_TARGET,
                "input": "*** Begin Patch\n*** Add File: bad-pin.txt\n+nope\n*** End Patch\n"
            }),
        )
        .await;

    assert!(error.contains("pinned server certificate mismatch"));
}
