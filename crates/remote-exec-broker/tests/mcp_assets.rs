mod support;

use axum::http::StatusCode;
use remote_exec_proto::rpc::RpcErrorBody;
use rmcp::model::PaginatedRequestParams;

#[tokio::test]
async fn apply_patch_returns_plain_text_plus_empty_structured_content() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
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
    assert_eq!(result.structured_content, serde_json::json!({}));
}

#[tokio::test]
async fn list_targets_returns_cached_daemon_info_and_null_for_unavailable_targets() {
    let fixture = support::spawn_broker_with_reverse_ordered_targets().await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.text_output,
        "Configured targets:\n- builder-a: linux/x86_64, host=builder-a-host, version=0.1.0, pty=yes\n- builder-b"
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
                        "supports_pty": true
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
async fn view_image_returns_input_image_content_and_structured_content() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
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
async fn view_image_returns_text_only_errors_without_input_image_content() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    fixture
        .set_image_read_response(support::StubImageReadResponse::Error {
            status: StatusCode::BAD_REQUEST,
            body: RpcErrorBody {
                code: "image_missing".to_string(),
                message:
                    "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)"
                        .to_string(),
            },
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
    assert_eq!(
        result.text_output,
        "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)"
    );
    assert_eq!(
        result.raw_content,
        vec![serde_json::json!({
            "type": "text",
            "text": "unable to locate image at `/tmp/chart.png`: No such file or directory (os error 2)"
        })]
    );
}

#[tokio::test]
async fn view_image_invalid_detail_matches_daemon_message() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    fixture
        .set_image_read_response(support::StubImageReadResponse::Error {
            status: StatusCode::BAD_REQUEST,
            body: RpcErrorBody {
                code: "invalid_detail".to_string(),
                message:
                    "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`"
                        .to_string(),
            },
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
    assert_eq!(
        result.text_output,
        "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `low`"
    );
}

#[tokio::test]
async fn list_targets_is_advertised_as_read_only() {
    let fixture = support::spawn_broker_with_stub_daemon().await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams {
            meta: None,
            cursor: None,
        }))
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
    let fixture = support::spawn_broker_with_stub_daemon().await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams {
            meta: None,
            cursor: None,
        }))
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
