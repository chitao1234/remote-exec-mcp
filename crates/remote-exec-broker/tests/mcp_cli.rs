#[path = "support/mod.rs"]
mod support;

#[tokio::test]
async fn remote_exec_cli_lists_targets_from_broker_config() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&fixture.config_path)
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

#[tokio::test]
async fn remote_exec_cli_rejects_removed_broker_bin_flag() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&fixture.config_path)
        .arg("--broker-bin")
        .arg("ignored")
        .arg("list-targets")
        .output()
        .await
        .unwrap();

    assert!(
        !output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--broker-bin"),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_ignores_mcp_transport_when_using_broker_config() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let mut config = tokio::fs::read_to_string(&fixture.config_path)
        .await
        .unwrap();
    config.push_str(
        r#"
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "/mcp"
"#,
    );
    tokio::fs::write(&fixture.config_path, config)
        .await
        .unwrap();

    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&fixture.config_path)
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
