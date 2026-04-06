mod support;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
