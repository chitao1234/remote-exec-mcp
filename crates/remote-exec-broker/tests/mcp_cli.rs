#[path = "support/mod.rs"]
mod support;

use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn run_cli(args: &[&str]) -> std::process::Output {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
            .args(&args)
            .output()
    })
    .await
    .unwrap()
    .unwrap()
}

async fn run_cli_with_stdin(args: &[&str], stdin: &str) -> std::process::Output {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let stdin = stdin.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;

        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output()
    })
    .await
    .unwrap()
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

#[cfg(target_os = "macos")]
#[tokio::test]
async fn remote_exec_cli_local_macos_shells_load_login_profiles() {
    let fixture = support::spawners::spawn_broker_config_local_only().await;
    let home = tempfile::tempdir().unwrap();
    for profile in [".profile", ".bash_profile", ".zprofile"] {
        std::fs::write(home.path().join(profile), "export LOGIN_SENTINEL=loaded\n").unwrap();
    }

    for shell in ["/bin/sh", "/bin/bash", "/bin/zsh"] {
        for login in [true, false] {
            let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec"))
                .env("HOME", home.path())
                .env("ZDOTDIR", home.path())
                .env_remove("LOGIN_SENTINEL")
                .arg("--broker-config")
                .arg(&fixture.config_path)
                .args(["--json", "exec-command", "local", "--shell", shell])
                .arg(if login { "--login" } else { "--no-login" })
                .args([
                    "--yield-time-ms",
                    "10000",
                    "printf '%s' \"${LOGIN_SENTINEL-unset}\"",
                ])
                .output()
                .await
                .unwrap();
            assert_exit_code(&output, 0);
            let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(response["structured_content"]["exit_code"], 0);
            assert_eq!(
                response["structured_content"]["output"],
                if login { "loaded" } else { "unset" },
                "shell={shell}, login={login}: {response}"
            );
        }
    }
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
        DEFAULT_TEST_TARGET
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
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "apply-patch",
        DEFAULT_TEST_TARGET,
        "--input-file",
        missing_patch.to_str().unwrap(),
    ])
    .await;

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
    let output = run_cli(&[
        "--broker-config",
        missing_config.to_str().unwrap(),
        "list-targets",
    ])
    .await;

    assert_exit_code(&output, 3);
}

#[tokio::test]
async fn remote_exec_cli_returns_connection_code_for_broker_transport_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });

    let output = run_cli(&["--broker-url", &url, "list-targets"]).await;

    assert_exit_code(&output, 4);
}

#[tokio::test]
async fn remote_exec_cli_returns_tool_code_for_tool_errors() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "exec-command",
        "missing-target",
        "printf nope",
    ])
    .await;

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
        DEFAULT_TEST_TARGET
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
    assert!(write_stdin_stdout.contains("write-stdin <SESSION_ID> [CHARS]"));
    assert!(write_stdin_stdout.contains("write-stdin <TARGET> <SESSION_ID> [CHARS]"));
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
    assert!(stdout.contains("<TARGET> <CMD>"));
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

    let forward = format!("tcp:127.0.0.1:0={echo_addr}");
    let open_output = run_cli(&[
        "--broker-url",
        &fixture.url,
        "--json",
        "forward-ports",
        "open",
        "--listen-side",
        "local",
        "--connect-side",
        "local",
        "--forward",
        &forward,
    ])
    .await;

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

    let list_output = run_cli(&[
        "--broker-url",
        &fixture.url,
        "--json",
        "forward-ports",
        "list",
        "--forward-id",
        &forward_id,
    ])
    .await;

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

    let close_output = run_cli(&[
        "--broker-url",
        &fixture.url,
        "--json",
        "forward-ports",
        "close",
        "--forward-id",
        &forward_id,
    ])
    .await;

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

async fn enable_file_tools(config_path: &std::path::Path) {
    let mut config = tokio::fs::read_to_string(config_path).await.unwrap();
    config.push_str("\n[tools.file]\nread = true\nwrite = true\nedit = true\n");
    tokio::fs::write(config_path, config).await.unwrap();
}

#[tokio::test]
async fn remote_exec_cli_reads_writes_and_edits_files() {
    let fixture = support::spawners::spawn_broker_config_local_only().await;
    enable_file_tools(&fixture.config_path).await;
    let file_path = fixture._tempdir.path().join("cli-file.txt");
    let file_path = file_path.to_str().unwrap();
    let config_path = fixture.config_path.to_str().unwrap();

    let write = run_cli(&[
        "--broker-config",
        config_path,
        "write",
        "local",
        file_path,
        "alpha\nbeta\n",
    ])
    .await;
    assert!(
        write.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&write.stdout),
        String::from_utf8_lossy(&write.stderr)
    );
    assert_eq!(
        tokio::fs::read_to_string(file_path).await.unwrap(),
        "alpha\nbeta\n"
    );

    let read = run_cli(&["--broker-config", config_path, "read", "local", file_path]).await;
    assert!(
        read.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&read.stdout),
        String::from_utf8_lossy(&read.stderr)
    );
    let read_stdout = String::from_utf8(read.stdout).unwrap();
    assert!(read_stdout.contains("1: alpha"));
    assert!(read_stdout.contains("2: beta"));

    let edit = run_cli(&[
        "--broker-config",
        config_path,
        "edit",
        "local",
        file_path,
        "beta",
        "gamma",
    ])
    .await;
    assert!(
        edit.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&edit.stdout),
        String::from_utf8_lossy(&edit.stderr)
    );
    assert_eq!(
        tokio::fs::read_to_string(file_path).await.unwrap(),
        "alpha\ngamma\n"
    );
}

#[tokio::test]
async fn remote_exec_cli_reads_write_content_from_stdin() {
    let fixture = support::spawners::spawn_broker_config_local_only().await;
    enable_file_tools(&fixture.config_path).await;
    let file_path = fixture._tempdir.path().join("cli-stdin.txt");
    let file_path = file_path.to_str().unwrap();

    let output = run_cli_with_stdin(
        &[
            "--broker-config",
            fixture.config_path.to_str().unwrap(),
            "write",
            "local",
            file_path,
            "--content-file",
            "-",
        ],
        "from stdin\n",
    )
    .await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        tokio::fs::read_to_string(file_path).await.unwrap(),
        "from stdin\n"
    );
}

#[tokio::test]
async fn remote_exec_cli_reports_disabled_file_tools() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "read",
        DEFAULT_TEST_TARGET,
        "sample.txt",
    ])
    .await;

    assert_exit_code(&output, 5);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tools.file.read = true"),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
}

#[tokio::test]
async fn remote_exec_cli_help_lists_file_tools_and_positional_paths() {
    let output = run_cli(&["--help"]).await;
    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    for command in ["read", "write", "edit"] {
        assert!(
            stdout.contains(command),
            "missing `{command}` in CLI help:\n{stdout}"
        );
    }

    let view_image = run_cli(&["view-image", "--help"]).await;
    assert!(
        view_image.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&view_image.stdout),
        String::from_utf8_lossy(&view_image.stderr)
    );
    let view_image_stdout = String::from_utf8(view_image.stdout).unwrap();
    assert!(view_image_stdout.contains("<TARGET> <PATH>"));
}

#[tokio::test]
async fn remote_exec_cli_accepts_mcp_style_tool_name_aliases() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "--json",
        "list_targets",
    ])
    .await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["structured_content"]["targets"][0]["name"],
        DEFAULT_TEST_TARGET
    );
}

#[tokio::test]
async fn remote_exec_cli_list_and_cp_aliases_work() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    for alias in ["list", "ls"] {
        let output = run_cli(&[
            "--broker-config",
            fixture.config_path.to_str().unwrap(),
            "--json",
            alias,
        ])
        .await;
        assert!(
            output.status.success(),
            "alias={alias}\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response["structured_content"]["targets"][0]["name"],
            DEFAULT_TEST_TARGET
        );
    }

    let cp_help = run_cli(&["cp", "--help"]).await;
    assert!(
        cp_help.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&cp_help.stdout),
        String::from_utf8_lossy(&cp_help.stderr)
    );
    assert!(
        String::from_utf8(cp_help.stdout)
            .unwrap()
            .contains("Endpoint format: <target>:<absolute-path>")
    );
}

#[tokio::test]
async fn remote_exec_cli_write_stdin_accepts_session_first_and_target_first_forms() {
    let fixture = support::spawners::spawn_broker_config_local_only().await;
    let config_path = fixture.config_path.to_str().unwrap();
    let session_id = "sess_00000000000000000000000000000000";

    for args in [
        vec!["write-stdin", session_id],
        vec!["write-stdin", session_id, "input"],
        vec!["write-stdin", "local", session_id],
        vec!["write-stdin", "local", session_id, "input"],
    ] {
        let mut command = vec!["--broker-config", config_path];
        command.extend(args.iter().copied());
        let output = run_cli(&command).await;
        assert_exit_code(&output, 5);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(session_id),
            "args={args:?}\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
async fn remote_exec_cli_write_stdin_rejects_invalid_session_after_target() {
    let fixture = support::spawners::spawn_broker_config_local_only().await;
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "write-stdin",
        "local",
        "not-a-session",
    ])
    .await;

    assert_exit_code(&output, 2);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a public session id"),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_apply_patch_reads_patch_from_stdin() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = run_cli_with_stdin(
        &[
            "--broker-config",
            fixture.config_path.to_str().unwrap(),
            "apply-patch",
            DEFAULT_TEST_TARGET,
        ],
        "*** Begin Patch\n*** End Patch\n",
    )
    .await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_apply_patch_accepts_positional_input() {
    let fixture = support::spawners::spawn_broker_config_with_stub_daemon().await;
    let output = run_cli(&[
        "--broker-config",
        fixture.config_path.to_str().unwrap(),
        "apply-patch",
        DEFAULT_TEST_TARGET,
        "*** Begin Patch\n*** End Patch\n",
    ])
    .await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn remote_exec_cli_write_reads_content_from_stdin_when_omitted() {
    let fixture = support::spawners::spawn_broker_config_local_only().await;
    enable_file_tools(&fixture.config_path).await;
    let file_path = fixture._tempdir.path().join("cli-implicit-stdin.txt");
    let file_path = file_path.to_str().unwrap();

    let output = run_cli_with_stdin(
        &[
            "--broker-config",
            fixture.config_path.to_str().unwrap(),
            "write",
            "local",
            file_path,
        ],
        "implicit stdin\n",
    )
    .await;

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        tokio::fs::read_to_string(file_path).await.unwrap(),
        "implicit stdin\n"
    );
}
