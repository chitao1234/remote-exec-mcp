#[path = "support/mod.rs"]
mod support;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn run_cli(args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .args(args)
        .output()
        .await
        .unwrap()
}

fn assert_exit_code(output: &std::process::Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_lists_targets_from_broker_config() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "--json",
        "list-targets",
    ])
    .await;

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
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "--broker-bin",
        "ignored",
        "list-targets",
    ])
    .await;

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
async fn remote_exec_cli_returns_usage_code_for_input_errors() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let missing_patch = fixture._tempdir.path().join("missing.patch");
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&fixture.config_path)
        .arg("apply-patch")
        .arg("--target")
        .arg("builder-a")
        .arg("--input-file")
        .arg(&missing_patch)
        .output()
        .await
        .unwrap();

    assert_exit_code(&output, 2);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("reading"),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_returns_config_code_for_config_errors() {
    let tempdir = tempfile::tempdir().unwrap();
    let missing_config = tempdir.path().join("missing-broker.toml");
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&missing_config)
        .arg("list-targets")
        .output()
        .await
        .unwrap();

    assert_exit_code(&output, 3);
}

#[tokio::test]
async fn remote_exec_cli_returns_connection_code_for_broker_transport_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    drop(listener);

    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-url")
        .arg(url)
        .arg("list-targets")
        .output()
        .await
        .unwrap();

    assert_exit_code(&output, 4);
}

#[tokio::test]
async fn remote_exec_cli_returns_tool_code_for_tool_errors() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-config")
        .arg(&fixture.config_path)
        .arg("exec-command")
        .arg("--target")
        .arg("missing-target")
        .arg("printf nope")
        .output()
        .await
        .unwrap();

    assert_exit_code(&output, 5);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing-target"),
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

#[tokio::test]
async fn remote_exec_cli_help_lists_forward_ports() {
    let output = run_cli(&["--help"]).await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("forward-ports"),
        "stdout:\n{}\n\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_help_describes_connection_modes() {
    let output = run_cli(&["--help"]).await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "Connect to a remote-exec broker over an in-process config or streamable HTTP"
        )
    );
    assert!(stdout.contains("Load a broker config and call broker tools in-process"));
    assert!(stdout.contains("Connect to a running broker over streamable HTTP"));
}

#[tokio::test]
async fn remote_exec_cli_transfer_help_documents_endpoint_syntax() {
    let output = run_cli(&["transfer-files", "--help"]).await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("<target>:<absolute-path>"));
    assert!(stdout.contains("Repeat --source to transfer multiple inputs"));
}

#[tokio::test]
async fn remote_exec_cli_help_documents_stdin_backed_inputs() {
    let apply_patch = run_cli(&["apply-patch", "--help"]).await;
    assert!(
        apply_patch.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&apply_patch.stdout),
        String::from_utf8_lossy(&apply_patch.stderr)
    );
    let apply_patch_stdout = String::from_utf8(apply_patch.stdout).unwrap();
    assert!(apply_patch_stdout.contains("use `-` to read from stdin"));

    let write_stdin = run_cli(&["write-stdin", "--help"]).await;
    assert!(
        write_stdin.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&write_stdin.stdout),
        String::from_utf8_lossy(&write_stdin.stderr)
    );
    let write_stdin_stdout = String::from_utf8(write_stdin.stdout).unwrap();
    assert!(write_stdin_stdout.contains("use `-` to read from stdin"));
}

#[tokio::test]
async fn remote_exec_cli_exec_alias_shows_exec_command_help() {
    let output = run_cli(&["exec", "--help"]).await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Run a command on a configured target machine."));
    assert!(stdout.contains("--target <TARGET>"));
}

#[tokio::test]
async fn remote_exec_cli_forward_ports_opens_lists_and_closes_local_tcp_forward() {
    let fixture = support::spawners::spawn_streamable_http_broker_with_stub_daemon().await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = echo_listener.accept().await.unwrap();
        let mut buf = vec![0u8; 64];
        let read = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..read]).await.unwrap();
    });

    let open_output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-url")
        .arg(&fixture.url)
        .arg("--json")
        .arg("forward-ports")
        .arg("open")
        .arg("--listen-side")
        .arg("local")
        .arg("--connect-side")
        .arg("local")
        .arg("--forward")
        .arg(format!("tcp:127.0.0.1:0={echo_addr}"))
        .output()
        .await
        .unwrap();

    assert!(
        open_output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&open_output.stdout),
        String::from_utf8_lossy(&open_output.stderr)
    );

    let open_stdout = String::from_utf8(open_output.stdout).unwrap();
    let open_response: serde_json::Value = serde_json::from_str(&open_stdout).unwrap();
    let forward_id = open_response["structured_content"]["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open_response["structured_content"]["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(listen_endpoint, "127.0.0.1:0");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    stream.write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello");

    let list_output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-url")
        .arg(&fixture.url)
        .arg("--json")
        .arg("forward-ports")
        .arg("list")
        .arg("--forward-id")
        .arg(&forward_id)
        .output()
        .await
        .unwrap();

    assert!(
        list_output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&list_output.stdout),
        String::from_utf8_lossy(&list_output.stderr)
    );

    let list_stdout = String::from_utf8(list_output.stdout).unwrap();
    let list_response: serde_json::Value = serde_json::from_str(&list_stdout).unwrap();
    assert_eq!(
        list_response["structured_content"]["forwards"][0]["status"],
        "open"
    );

    let close_output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
        .arg("--broker-url")
        .arg(&fixture.url)
        .arg("--json")
        .arg("forward-ports")
        .arg("close")
        .arg("--forward-id")
        .arg(&forward_id)
        .output()
        .await
        .unwrap();

    assert!(
        close_output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&close_output.stdout),
        String::from_utf8_lossy(&close_output.stderr)
    );

    let close_stdout = String::from_utf8(close_output.stdout).unwrap();
    let close_response: serde_json::Value = serde_json::from_str(&close_stdout).unwrap();
    assert_eq!(
        close_response["structured_content"]["forwards"][0]["status"],
        "closed"
    );
}
