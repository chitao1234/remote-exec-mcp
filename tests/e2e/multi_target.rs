mod support;

use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn sessions_are_isolated_per_target() {
    let cluster = support::spawn_cluster().await;

    let started = cluster
        .broker
        .call_tool(
            "exec_command",
            support::long_running_tty_exec_input("builder-a"),
        )
        .await;

    let session_id = started.structured_content["session_id"].as_str().unwrap();
    let polled = cluster
        .broker
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-a",
                "chars": "",
                "yield_time_ms": 250
            }),
        )
        .await;
    assert_eq!(polled.structured_content["target"], "builder-a");

    let mismatch = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-b",
                "chars": ""
            }),
        )
        .await;
    assert!(mismatch.contains("does not belong"), "mismatch: {mismatch}");
}

#[tokio::test]
async fn patch_and_image_calls_only_touch_the_selected_target() {
    let cluster = support::spawn_cluster().await;
    support::write_png(&cluster.daemon_b.workdir.join("builder-b.png"), 12, 8).await;

    cluster
        .broker
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: marker.txt\n+builder-a\n*** End Patch\n"
            }),
        )
        .await;

    let image = cluster
        .broker
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "builder-b",
                "path": "builder-b.png",
                "detail": "original"
            }),
        )
        .await;

    assert!(cluster.daemon_a.workdir.join("marker.txt").exists());
    assert!(!cluster.daemon_b.workdir.join("marker.txt").exists());
    assert_eq!(image.structured_content["target"], "builder-b");
    assert_eq!(image.raw_content[0]["type"], "input_image");

    let wrong_target = cluster
        .broker
        .call_tool_error(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "builder-b.png",
                "detail": "original"
            }),
        )
        .await;
    assert!(
        wrong_target.contains("No such file")
            || wrong_target.contains("os error 2")
            || wrong_target.contains("internal_error"),
        "wrong target error: {wrong_target}"
    );
}

#[tokio::test]
async fn sessions_are_invalidated_after_daemon_restart() {
    let mut cluster = support::spawn_cluster().await;
    let started = cluster
        .broker
        .call_tool(
            "exec_command",
            support::long_running_tty_exec_input("builder-a"),
        )
        .await;
    let session_id = started.structured_content["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    cluster.daemon_a.restart().await;

    let invalidated = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-a",
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;
    assert_eq!(
        invalidated,
        format!("write_stdin failed: Unknown process id {session_id}"),
        "restart invalidation error: {invalidated}"
    );

    let unknown = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": started.structured_content["session_id"],
                "target": "builder-a",
                "chars": ""
            }),
        )
        .await;
    assert_eq!(
        unknown,
        format!("write_stdin failed: Unknown process id {session_id}"),
        "unknown session error: {unknown}"
    );
}

#[tokio::test]
async fn forward_ports_reports_daemon_listen_port_conflicts_cleanly() {
    let cluster = support::spawn_cluster().await;
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();

    let error = cluster
        .broker
        .call_tool_error(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": occupied_addr.to_string(),
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;

    assert!(
        error.contains("opening tcp listener on `builder-a`")
            && error.contains(&occupied_addr.to_string()),
        "expected clean bind failure, got: {error}"
    );
}

#[tokio::test]
async fn forward_ports_release_remote_listeners_when_broker_stops() {
    let mut cluster = support::spawn_cluster().await;
    let listen_addr = support::allocate_addr();

    let open = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": listen_addr.to_string(),
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    assert_eq!(
        open.structured_content["forwards"][0]["listen_endpoint"],
        listen_addr.to_string()
    );

    cluster.broker.stop().await;

    let rebound = cluster
        .daemon_a
        .port_listen(
            &listen_addr.to_string(),
            remote_exec_proto::rpc::PortForwardProtocol::Tcp,
        )
        .await;
    assert_eq!(rebound.endpoint, listen_addr.to_string());
    cluster.daemon_a.port_listen_close(&rebound.bind_id).await;
}

#[tokio::test]
async fn forward_ports_fail_cleanly_after_daemon_restart_and_can_reopen() {
    let mut cluster = support::spawn_cluster().await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match echo_listener.accept().await {
                Ok(value) => value,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    let read = match stream.read(&mut buf).await {
                        Ok(0) => return,
                        Ok(read) => read,
                        Err(_) => return,
                    };
                    if stream.write_all(&buf[..read]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    let open = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    let mut before_restart = tokio::net::TcpStream::connect(&listen_endpoint)
        .await
        .unwrap();
    before_restart.write_all(b"before").await.unwrap();
    let mut echoed = [0u8; 6];
    before_restart.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"before");
    drop(before_restart);

    cluster.daemon_a.restart().await;

    let after_restart = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(&listen_endpoint),
    )
    .await;
    assert!(
        after_restart.is_err() || after_restart.unwrap().is_err(),
        "stale forwarded listener should stop accepting connections after daemon restart"
    );

    let failed = wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "failed",
        Duration::from_secs(5),
    )
    .await;
    if let Some(failed) = failed {
        assert_eq!(failed["status"], "failed");
        let last_error = failed["last_error"].as_str().unwrap_or_default();
        assert!(
            last_error.contains("daemon transport error")
                || last_error.contains("port_bind_closed")
                || last_error.contains("port_accept_failed"),
            "unexpected restart failure error: {last_error}"
        );
    }

    let reopened = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let reopened_id = reopened.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let reopened_endpoint = reopened.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    let mut after_reopen = tokio::net::TcpStream::connect(&reopened_endpoint)
        .await
        .unwrap();
    after_reopen.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    after_reopen.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");

    let closed = cluster
        .broker
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [reopened_id]
            }),
        )
        .await;
    assert_eq!(closed.structured_content["forwards"][0]["status"], "closed");
}

#[cfg(unix)]
#[tokio::test]
async fn exec_command_preserves_output_after_external_pipeline_steps() {
    let cluster = support::spawn_cluster().await;

    let result = cluster
        .broker
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf 'marker\\n'; printf 'external\\n' | cat; printf 'done\\n'",
                "shell": "/bin/sh",
                "tty": false,
                "yield_time_ms": 10_000
            }),
        )
        .await;

    assert_eq!(result.structured_content["target"], "builder-a");
    assert_eq!(result.structured_content["exit_code"], 0);
    assert_eq!(
        result.structured_content["output"],
        serde_json::Value::String("marker\nexternal\ndone\n".to_string())
    );
}

#[tokio::test]
async fn transfer_files_copies_local_file_to_remote_exact_destination_path() {
    let cluster = support::spawn_cluster().await;
    let local_dir = tempfile::tempdir().unwrap();
    let source = local_dir.path().join("artifact.txt");
    std::fs::write(&source, "artifact\n").unwrap();
    let destination = cluster.daemon_a.workdir.join("releases/current.txt");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "builder-a",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "artifact\n");
    assert_eq!(
        result.structured_content["destination"]["target"],
        "builder-a"
    );
    assert!(
        !cluster
            .daemon_a
            .workdir
            .join("releases/artifact.txt")
            .exists()
    );
}

#[tokio::test]
async fn transfer_files_copies_remote_file_back_to_local() {
    let cluster = support::spawn_cluster().await;
    let source = cluster.daemon_a.workdir.join("build.log");
    std::fs::write(&source, "done\n").unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    let destination = local_dir.path().join("logs/build.log");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "done\n");
    assert_eq!(result.structured_content["source"]["target"], "builder-a");
}

#[tokio::test]
async fn transfer_files_moves_remote_directory_between_targets_without_basename_inference() {
    let cluster = support::spawn_cluster().await;
    let source_root = cluster.daemon_a.workdir.join("dist");
    std::fs::create_dir_all(source_root.join("empty")).unwrap();
    std::fs::create_dir_all(source_root.join("bin")).unwrap();
    std::fs::write(source_root.join("bin/tool.sh"), "#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();
    }
    let destination = cluster.daemon_b.workdir.join("release");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source_root.display().to_string()
                },
                "destination": {
                    "target": "builder-b",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(destination.join("empty").is_dir());
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(destination.join("bin/tool.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    assert!(!destination.join("dist").exists());
    assert_eq!(result.structured_content["source_type"], "directory");
}

#[tokio::test]
async fn transfer_files_bundles_multiple_local_sources_with_zstd_for_remote_destination() {
    let cluster = support::spawn_cluster().await;
    let local_dir = tempfile::tempdir().unwrap();
    let file_source = local_dir.path().join("alpha.txt");
    let directory_source = local_dir.path().join("tree");
    std::fs::write(&file_source, "alpha\n").unwrap();
    std::fs::create_dir_all(&directory_source).unwrap();
    std::fs::write(directory_source.join("nested.txt"), "nested\n").unwrap();
    let destination = cluster.daemon_a.workdir.join("bundle");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "sources": [
                    {
                        "target": "local",
                        "path": file_source.display().to_string()
                    },
                    {
                        "target": "local",
                        "path": directory_source.display().to_string()
                    }
                ],
                "destination": {
                    "target": "builder-a",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(
        std::fs::read_to_string(destination.join("alpha.txt")).unwrap(),
        "alpha\n"
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("tree/nested.txt")).unwrap(),
        "nested\n"
    );
    assert_eq!(result.structured_content["source_type"], "multiple");
    assert_eq!(
        result.structured_content["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

async fn wait_for_forward_status_timeout(
    broker: &support::BrokerFixture,
    forward_id: &str,
    status: &str,
    timeout: Duration,
) -> Option<serde_json::Value> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        let list = broker
            .call_tool(
                "forward_ports",
                serde_json::json!({
                    "action": "list",
                    "forward_ids": [forward_id]
                }),
            )
            .await;
        let entry = list.structured_content["forwards"][0].clone();
        if entry["status"] == status {
            return Some(entry);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    None
}
