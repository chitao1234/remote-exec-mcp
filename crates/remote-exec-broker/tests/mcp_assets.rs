mod support;

use axum::http::StatusCode;
use image::{ImageBuffer, Rgba};
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};
use remote_exec_test_support::test_helpers::utf16le_bom_bytes;
use rmcp::model::PaginatedRequestParams;

#[tokio::test]
async fn apply_patch_returns_plain_text_without_structured_output() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n",
                "workdir": "."
            }),
        )
        .await;

    assert!(
        result
            .text_output
            .contains("Success. Updated the following files:")
    );
    assert_eq!(result.structured_content, serde_json::Value::Null);
}

#[tokio::test]
async fn apply_patch_forwards_to_explicitly_enabled_insecure_http_target() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-xp",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello xp\n*** End Patch\n",
            }),
        )
        .await;

    assert!(
        result
            .text_output
            .contains("Success. Updated the following files:")
    );
    assert_eq!(result.structured_content, serde_json::Value::Null);
    assert_eq!(
        fixture
            .last_patch_request()
            .await
            .expect("patch request")
            .patch,
        "*** Begin Patch\n*** Add File: hello.txt\n+hello xp\n*** End Patch\n"
    );
}

#[tokio::test]
async fn apply_patch_forwards_to_http_target_with_bearer_auth() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_http_auth("shared-secret").await;
    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello auth\n*** End Patch\n",
            }),
        )
        .await;

    assert!(
        result
            .text_output
            .contains("Success. Updated the following files:")
    );
    assert_eq!(
        fixture
            .last_patch_request()
            .await
            .expect("patch request")
            .patch,
        "*** Begin Patch\n*** Add File: hello.txt\n+hello auth\n*** End Patch\n"
    );
}

#[tokio::test]
async fn malformed_patch_footer_is_rejected_by_stub_daemon() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: missing-footer.txt\n+hello\n"
            }),
        )
        .await;

    assert!(
        error.contains("invalid patch footer"),
        "unexpected malformed patch footer error: {error}"
    );
}

#[tokio::test]
async fn list_targets_returns_cached_daemon_info_and_null_for_unavailable_targets() {
    let fixture = support::spawners::spawn_broker_with_reverse_ordered_targets().await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.text_output,
        "Configured targets:\n- builder-a: linux/x86_64, host=builder-a-host, version=0.1.0, pty=yes, forward_ports=no\n- builder-b: unavailable (no cached daemon info)"
    );
    assert_eq!(
        result.structured_content,
        serde_json::json!({
            "targets": [
                {
                    "name": "builder-a",
                    "daemon_info": {
                        "daemon_version": "0.1.0",
                        "hostname": "builder-a-host",
                        "platform": "linux",
                        "arch": "x86_64",
                        "supports_pty": true,
                        "supports_port_forward": false
                    }
                },
                {
                    "name": "builder-b",
                    "daemon_info": null
                }
            ]
        })
    );
}

#[tokio::test]
async fn list_targets_omits_structured_content_when_broker_disables_it() {
    let fixture =
        support::spawners::spawn_broker_with_stub_daemon_and_structured_content_disabled().await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.text_output,
        "Configured targets:\n- builder-a: linux/x86_64, host=builder-a-host, version=0.1.0, pty=yes, forward_ports=no"
    );
    assert_eq!(result.structured_content, serde_json::Value::Null);
}

#[tokio::test]
async fn list_targets_formats_windows_metadata_and_truthful_pty_support() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon_platform("windows", false).await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.text_output,
        "Configured targets:\n- builder-a: windows/x86_64, host=builder-a-host, version=0.1.0, pty=no, forward_ports=no"
    );
}

#[tokio::test]
async fn list_targets_reports_port_forward_protocol_version_when_available() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(4).await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.structured_content["targets"][0]["daemon_info"]["port_forward_protocol_version"],
        serde_json::json!(4)
    );
    assert!(
        result.text_output.contains("forward_ports=yes")
            && result.text_output.contains("forward_protocol=v4"),
        "unexpected text: {}",
        result.text_output
    );
}

#[tokio::test]
async fn list_targets_includes_enabled_local_target() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(result.structured_content["targets"][0]["name"], "local");
    assert_eq!(
        result.structured_content["targets"][0]["daemon_info"]["platform"],
        std::env::consts::OS
    );
    assert!(
        result
            .text_output
            .starts_with("Configured targets:\n- local:"),
        "unexpected text output: {}",
        result.text_output
    );
}

#[tokio::test]
async fn apply_patch_runs_against_enabled_local_target() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;
    let workdir = fixture.local_workdir();
    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "local",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello local\n*** End Patch\n",
                "workdir": workdir.display().to_string()
            }),
        )
        .await;

    assert!(
        result
            .text_output
            .contains("Success. Updated the following files:")
    );
    assert_eq!(
        std::fs::read_to_string(workdir.join("hello.txt")).unwrap(),
        "hello local\n"
    );
}

#[tokio::test]
async fn apply_patch_local_target_can_autodetect_existing_target_encoding_when_enabled() {
    let fixture =
        support::spawners::spawn_broker_with_local_target_apply_patch_encoding_autodetect().await;
    let workdir = fixture.local_workdir();
    let path = workdir.join("utf16.txt");
    std::fs::write(&path, utf16le_bom_bytes("hello\r\nworld\r\n")).unwrap();

    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "local",
                "input": concat!(
                    "*** Begin Patch\n",
                    "*** Update File: utf16.txt\n",
                    "@@\n",
                    "-hello\n",
                    "+hello local\n",
                    "*** End Patch\n",
                ),
                "workdir": workdir.display().to_string()
            }),
        )
        .await;

    assert!(result.text_output.contains("M utf16.txt"));
    assert_eq!(
        std::fs::read(path).unwrap(),
        utf16le_bom_bytes("hello local\r\nworld\r\n")
    );
}

#[tokio::test]
async fn view_image_returns_input_image_content_and_structured_content() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png",
                "detail": "original"
            }),
        )
        .await;

    assert_eq!(result.raw_content[0]["type"], "input_image");
    assert_eq!(
        result.raw_content[0]["image_url"],
        "data:image/png;base64,AAAA"
    );
    assert_eq!(result.structured_content["target"], "builder-a");
    assert_eq!(result.structured_content["detail"], "original");
}

#[tokio::test]
async fn view_image_reads_from_enabled_local_target() {
    let fixture = support::spawners::spawn_broker_with_local_target().await;
    let image_path = fixture.local_workdir().join("chart.png");
    let image = ImageBuffer::<Rgba<u8>, _>::from_pixel(2, 2, Rgba([0, 128, 255, 255]));
    image.save(&image_path).unwrap();

    let result = fixture
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "local",
                "path": image_path.display().to_string(),
                "detail": "original"
            }),
        )
        .await;

    assert_eq!(result.raw_content[0]["type"], "input_image");
    assert_eq!(result.structured_content["target"], "local");
    assert_eq!(result.structured_content["detail"], "original");
}

#[tokio::test]
async fn view_image_keeps_input_image_content_when_broker_disables_structured_content() {
    let fixture =
        support::spawners::spawn_broker_with_stub_daemon_and_structured_content_disabled().await;
    let result = fixture
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png",
                "detail": "original"
            }),
        )
        .await;

    assert_eq!(result.raw_content[0]["type"], "input_image");
    assert_eq!(
        result.raw_content[0]["image_url"],
        "data:image/png;base64,AAAA"
    );
    assert_eq!(result.structured_content, serde_json::Value::Null);
}

#[tokio::test]
async fn view_image_returns_text_only_errors_without_input_image_content() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    fixture
        .set_image_read_response(support::stub_daemon::StubImageReadResponse::Error {
            status: StatusCode::BAD_REQUEST,
            body: RpcErrorBody::new(
                RpcErrorCode::ImageMissing,
                "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)",
            ),
        })
        .await;

    let result = fixture
        .raw_tool_result(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png"
            }),
        )
        .await;

    assert!(result.is_error);
    support::assert_correlated_tool_error(
        &result.text_output,
        "view_image",
        Some("builder-a"),
        "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)",
    );
    assert_eq!(
        result.raw_content,
        vec![serde_json::json!({
            "type": "text",
            "text": result.text_output
        })]
    );
}

#[tokio::test]
async fn view_image_invalid_detail_matches_daemon_message() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    fixture
        .set_image_read_response(support::stub_daemon::StubImageReadResponse::Error {
            status: StatusCode::BAD_REQUEST,
            body: RpcErrorBody::new(
                RpcErrorCode::InvalidDetail,
                "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`",
            ),
        })
        .await;

    let result = fixture
        .raw_tool_result(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png",
                "detail": "low"
            }),
        )
        .await;

    assert!(result.is_error);
    support::assert_correlated_tool_error(
        &result.text_output,
        "view_image",
        Some("builder-a"),
        "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`",
    );
}

#[tokio::test]
async fn list_targets_is_advertised_as_read_only() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams::default()))
        .await
        .expect("list tools");

    let list_targets = tools
        .tools
        .into_iter()
        .find(|tool| tool.name.as_ref() == "list_targets")
        .expect("list_targets tool");

    assert_eq!(
        list_targets
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
        Some(true)
    );
}

#[tokio::test]
async fn view_image_is_advertised_as_read_only() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams::default()))
        .await
        .expect("list tools");

    let view_image = tools
        .tools
        .into_iter()
        .find(|tool| tool.name.as_ref() == "view_image")
        .expect("view_image tool");

    assert_eq!(
        view_image
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
        Some(true)
    );
}
