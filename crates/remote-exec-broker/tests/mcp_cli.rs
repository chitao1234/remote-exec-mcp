#[path = "support/mod.rs"]
mod support;

#[tokio::test]
async fn remote_exec_cli_lists_targets_over_stdio() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&fixture.config_path)
        .arg("--broker-bin")
        .arg(env!("CARGO_BIN_EXE_remote-exec-broker"))
        .arg("--json")
        .arg("list-targets")
        .output()
        .await
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let response: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(response["is_error"], false);
    assert_eq!(
        response["structured_content"]["targets"][0]["name"],
        "builder-a"
    );
}
