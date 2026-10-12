#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use image::{ImageBuffer, Rgba};
use remote_exec_broker::ToolResponse;
use support::cpp_daemon::{CPP_TARGET, CppDaemonBrokerFixture};

const FAST_YIELD_DAEMON_CONFIG: &str = "yield_time_exec_command_default_ms = 1\n\
yield_time_exec_command_max_ms = 5000\n\
yield_time_exec_command_min_ms = 1\n\
yield_time_write_stdin_poll_default_ms = 1\n\
yield_time_write_stdin_poll_max_ms = 5000\n\
yield_time_write_stdin_poll_min_ms = 1\n\
yield_time_write_stdin_input_default_ms = 1\n\
yield_time_write_stdin_input_max_ms = 5000\n\
yield_time_write_stdin_input_min_ms = 1\n";

#[tokio::test]
async fn public_tools_smoke_against_real_cpp_daemon() {
    let Some(fixture) =
        CppDaemonBrokerFixture::spawn_with_daemon_config(FAST_YIELD_DAEMON_CONFIG).await
    else {
        return;
    };

    assert_list_targets_smoke(&fixture).await;
    assert_exec_command_smoke(&fixture).await;
    assert_write_stdin_smoke(&fixture).await;
    assert_apply_patch_smoke(&fixture).await;
    assert_view_image_smoke(&fixture).await;
    assert_transfer_files_smoke(&fixture).await;
}

async fn assert_list_targets_smoke(fixture: &CppDaemonBrokerFixture) {
    let result = fixture
        .client
        .call_tool("list_targets", &serde_json::json!({}))
        .await
        .unwrap();
    assert_tool_ok(&result, "list_targets");

    let targets = result.structured_content["targets"].as_array().unwrap();
    let target = targets
        .iter()
        .find(|target| target["name"] == CPP_TARGET)
        .expect("C++ daemon target should be listed");
    assert_eq!(target["healthy"], true);
    let daemon_info = &target["daemon_info"];
    #[cfg(not(windows))]
    assert_eq!(daemon_info["supports_pty"], true);
    #[cfg(windows)]
    assert!(daemon_info["supports_pty"].as_bool().is_some());
    assert_eq!(daemon_info["supports_port_forward"], true);
    assert!(daemon_info.get("port_forward_protocol_version").is_none());
    assert_eq!(daemon_info["transfer_stream_protocol_version"], 2);
}

async fn assert_exec_command_smoke(fixture: &CppDaemonBrokerFixture) {
    let result = fixture
        .client
        .call_tool(
            "exec_command",
            &serde_json::json!({
                "target": CPP_TARGET,
                "cmd": echo_command(),
                "shell": test_shell(),
                "login": false,
                "tty": false,
                "yield_time_ms": 5000
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&result, "exec_command");
    assert_eq!(result.structured_content["target"], CPP_TARGET);
    assert_eq!(result.structured_content["exit_code"], 0);
    assert!(result.structured_content["session_id"].is_null());
    assert!(
        result.structured_content["output"]
            .as_str()
            .unwrap()
            .contains("cpp-smoke"),
        "unexpected exec output: {}",
        result.text_output
    );
}

async fn assert_write_stdin_smoke(fixture: &CppDaemonBrokerFixture) {
    let started = fixture
        .client
        .call_tool(
            "exec_command",
            &serde_json::json!({
                "target": CPP_TARGET,
                "cmd": stdin_command(),
                "shell": test_shell(),
                "login": false,
                "tty": stdin_requires_tty(),
                "yield_time_ms": 1
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&started, "exec_command");
    let session_id = started.structured_content["session_id"]
        .as_str()
        .expect("stdin smoke command should keep a running session")
        .to_string();

    let mut response = fixture
        .client
        .call_tool(
            "write_stdin",
            &serde_json::json!({
                "session_id": session_id,
                "target": CPP_TARGET,
                "chars": stdin_payload(),
                "yield_time_ms": 5000
            }),
        )
        .await
        .unwrap();
    let started_at = Instant::now();
    loop {
        assert_tool_ok(&response, "write_stdin");
        let output = response.structured_content["output"]
            .as_str()
            .unwrap_or_default();
        if output.contains("input:hello-from-stdin") {
            assert_eq!(response.structured_content["target"], CPP_TARGET);
            return;
        }
        if response.structured_content["exit_code"].is_number() {
            panic!("stdin smoke command completed without expected output: {output:?}");
        }
        if started_at.elapsed() >= Duration::from_secs(5) {
            panic!("stdin smoke command did not echo input; last output={output:?}");
        }
        response = fixture
            .client
            .call_tool(
                "write_stdin",
                &serde_json::json!({
                    "session_id": session_id,
                    "target": CPP_TARGET,
                    "chars": "",
                    "yield_time_ms": 100
                }),
            )
            .await
            .unwrap();
    }
}

async fn assert_apply_patch_smoke(fixture: &CppDaemonBrokerFixture) {
    let result = fixture
        .client
        .call_tool(
            "apply_patch",
            &serde_json::json!({
                "target": CPP_TARGET,
                "input": "*** Begin Patch\n*** Add File: patched-via-public-tool.txt\n+patched via public tool\n*** End Patch\n",
                "workdir": fixture.daemon_workdir().display().to_string()
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&result, "apply_patch");
    assert!(
        result
            .text_output
            .contains("Success. Updated the following files:"),
        "unexpected apply_patch output: {}",
        result.text_output
    );
    assert_eq!(
        std::fs::read_to_string(fixture.daemon_workdir().join("patched-via-public-tool.txt"))
            .unwrap(),
        "patched via public tool\n"
    );
}

async fn assert_view_image_smoke(fixture: &CppDaemonBrokerFixture) {
    let image_path = fixture.daemon_workdir().join("chart.png");
    let image = ImageBuffer::<Rgba<u8>, _>::from_pixel(2, 2, Rgba([0, 128, 255, 255]));
    image.save(&image_path).unwrap();

    let result = fixture
        .client
        .call_tool(
            "view_image",
            &serde_json::json!({
                "target": CPP_TARGET,
                "path": image_path.display().to_string(),
                "detail": "original"
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&result, "view_image");
    assert_eq!(result.structured_content["target"], CPP_TARGET);
    assert_eq!(result.structured_content["detail"], "original");
    assert_eq!(result.raw_content[0]["type"], "input_image");
    assert!(
        result.raw_content[0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
}

async fn assert_transfer_files_smoke(fixture: &CppDaemonBrokerFixture) {
    let local_source = fixture.local_workdir().join("local-source.txt");
    let remote_destination = fixture.daemon_workdir().join("remote-copy.txt");
    std::fs::write(&local_source, "local to cpp\n").unwrap();

    let local_to_cpp = fixture
        .client
        .call_tool(
            "transfer_files",
            &serde_json::json!({
                "source": {
                    "target": "local",
                    "path": local_source.display().to_string()
                },
                "destination": {
                    "target": CPP_TARGET,
                    "path": remote_destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&local_to_cpp, "transfer_files");
    assert_eq!(
        std::fs::read_to_string(&remote_destination).unwrap(),
        "local to cpp\n"
    );
    assert_eq!(
        local_to_cpp.structured_content["resolved_destination"]["target"],
        CPP_TARGET
    );

    let remote_source = fixture.daemon_workdir().join("remote-source.txt");
    let local_destination = fixture.local_workdir().join("local-copy.txt");
    std::fs::write(&remote_source, "cpp to local\n").unwrap();

    let cpp_to_local = fixture
        .client
        .call_tool(
            "transfer_files",
            &serde_json::json!({
                "source": {
                    "target": CPP_TARGET,
                    "path": remote_source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": local_destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&cpp_to_local, "transfer_files");
    assert_eq!(
        std::fs::read_to_string(&local_destination).unwrap(),
        "cpp to local\n"
    );
    assert_eq!(
        cpp_to_local.structured_content["source"]["target"],
        CPP_TARGET
    );
}

fn assert_tool_ok(result: &ToolResponse, tool: &str) {
    assert!(!result.is_error, "{tool} failed: {}", result.text_output);
}

const CPP_HIDDEN_FILE_TOOLS_BROKER_CONFIG: &str = "[tools.file]\n\
read = true\n\
write = true\n\
edit = true\n\
default_read_limit_lines = 20\n\
max_read_limit_lines = 200\n\
max_read_bytes = 1048576\n";

/// The hidden file tools must work end to end against the real C++ daemon:
/// broker routing, the daemon capability gate, and the daemon's own
/// read/write/edit semantics.
#[tokio::test]
async fn hidden_file_tools_work_against_real_cpp_daemon() {
    let Some(fixture) = CppDaemonBrokerFixture::spawn_with_daemon_and_broker_config(
        "",
        CPP_HIDDEN_FILE_TOOLS_BROKER_CONFIG,
    )
    .await
    else {
        return;
    };

    // The daemon now advertises file tool protocol version 1, which is what
    // lets the broker route these tools to it at all.
    let targets = fixture
        .client
        .call_tool("list_targets", &serde_json::json!({}))
        .await
        .unwrap();
    assert_tool_ok(&targets, "list_targets");
    let target = targets.structured_content["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|target| target["name"] == CPP_TARGET)
        .expect("C++ daemon target should be listed");
    assert_eq!(
        target["daemon_info"]["file_tool_protocol_version"], 1,
        "C++ daemon should advertise file tool protocol version 1"
    );

    let write = fixture
        .client
        .call_tool(
            "write",
            &serde_json::json!({
                "target": CPP_TARGET,
                "file_path": "cpp-file-tool.txt",
                "content": "alpha\nbeta\nalpha\n"
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&write, "write");
    assert!(
        write.text_output.contains("created"),
        "unexpected write output: {}",
        write.text_output
    );

    let read = fixture
        .client
        .call_tool(
            "read",
            &serde_json::json!({
                "target": CPP_TARGET,
                "file_path": "cpp-file-tool.txt",
                "limit": 2
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&read, "read");
    assert!(
        read.text_output.contains("1: alpha") && read.text_output.contains("2: beta"),
        "unexpected read output: {}",
        read.text_output
    );

    // A duplicated search string is ambiguous unless replace_all is set.
    let ambiguous = fixture
        .client
        .call_tool(
            "edit",
            &serde_json::json!({
                "target": CPP_TARGET,
                "file_path": "cpp-file-tool.txt",
                "old_string": "alpha",
                "new_string": "gamma"
            }),
        )
        .await
        .unwrap();
    assert!(ambiguous.is_error, "expected an ambiguity error");
    assert!(
        ambiguous.text_output.contains("file_old_string_ambiguous")
            || ambiguous.text_output.contains("matched 2 times"),
        "unexpected ambiguity output: {}",
        ambiguous.text_output
    );

    let edit = fixture
        .client
        .call_tool(
            "edit",
            &serde_json::json!({
                "target": CPP_TARGET,
                "file_path": "cpp-file-tool.txt",
                "old_string": "alpha",
                "new_string": "gamma",
                "replace_all": true
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&edit, "edit");
    assert!(
        edit.text_output.contains("2 replacements"),
        "unexpected edit output: {}",
        edit.text_output
    );

    let reread = fixture
        .client
        .call_tool(
            "read",
            &serde_json::json!({
                "target": CPP_TARGET,
                "file_path": "cpp-file-tool.txt",
                "limit": 10
            }),
        )
        .await
        .unwrap();
    assert_tool_ok(&reread, "read");
    assert!(
        reread.text_output.contains("1: gamma")
            && reread.text_output.contains("3: gamma")
            && !reread.text_output.contains("alpha"),
        "unexpected read output after edit: {}",
        reread.text_output
    );

    // A path outside the daemon workdir still resolves through the same
    // sandbox-checked path rules as the other tools.
    let missing = fixture
        .client
        .call_tool(
            "read",
            &serde_json::json!({
                "target": CPP_TARGET,
                "file_path": "does-not-exist.txt"
            }),
        )
        .await
        .unwrap();
    assert!(missing.is_error, "expected a missing-file error");
    assert!(
        missing.text_output.contains("file_missing"),
        "unexpected missing-file output: {}",
        missing.text_output
    );
}

#[cfg(windows)]
fn echo_command() -> &'static str {
    "echo cpp-smoke"
}

#[cfg(not(windows))]
fn echo_command() -> &'static str {
    "printf 'cpp-smoke\\n'"
}

#[cfg(windows)]
fn stdin_command() -> &'static str {
    "echo ready&set /P line=&call echo input:%line%"
}

#[cfg(not(windows))]
fn stdin_command() -> &'static str {
    "printf 'ready\\n'; IFS= read line; printf 'input:%s\\n' \"$line\""
}

#[cfg(windows)]
fn stdin_payload() -> &'static str {
    "hello-from-stdin\r\n"
}

#[cfg(not(windows))]
fn stdin_payload() -> &'static str {
    "hello-from-stdin\n"
}

#[cfg(windows)]
fn stdin_requires_tty() -> bool {
    false
}

#[cfg(not(windows))]
fn stdin_requires_tty() -> bool {
    true
}

#[cfg(windows)]
fn test_shell() -> &'static str {
    "cmd.exe"
}

#[cfg(not(windows))]
fn test_shell() -> &'static str {
    "/bin/sh"
}
