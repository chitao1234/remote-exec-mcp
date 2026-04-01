mod support;

#[tokio::test]
async fn sessions_are_isolated_per_target() {
    let cluster = support::spawn_cluster().await;

    let started = cluster
        .broker
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf hello; sleep 30",
                "tty": true,
                "yield_time_ms": 250
            }),
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
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf hello; sleep 30",
                "tty": true,
                "yield_time_ms": 250
            }),
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
