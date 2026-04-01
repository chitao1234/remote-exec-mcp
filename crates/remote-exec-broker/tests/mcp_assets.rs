mod support;

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
    assert_eq!(result.structured_content["detail"], "original");
}
