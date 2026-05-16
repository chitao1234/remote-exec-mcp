#[path = "support/mod.rs"]
mod support;

use remote_exec_broker::{Connection, RemoteExecClient};
use remote_exec_proto::public::{ExecCommandInput, ListTargetsInput};
use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;

#[tokio::test]
async fn broker_serves_tools_over_streamable_http() {
    let fixture = support::spawners::spawn_streamable_http_broker_with_stub_daemon().await;
    let client = RemoteExecClient::connect(Connection::StreamableHttp {
        url: fixture.url.clone(),
    })
    .await
    .unwrap();

    let targets = client
        .call_tool("list_targets", &ListTargetsInput::default())
        .await
        .unwrap();
    assert!(!targets.is_error);
    assert_eq!(
        targets.structured_content["targets"][0]["name"],
        DEFAULT_TEST_TARGET
    );

    let started = client
        .call_tool(
            "exec_command",
            &ExecCommandInput {
                target: DEFAULT_TEST_TARGET.to_string(),
                cmd: "printf ready".to_string(),
                workdir: None,
                shell: None,
                tty: true,
                yield_time_ms: Some(10),
                max_output_tokens: Some(200),
                login: None,
            },
        )
        .await
        .unwrap();
    assert!(!started.is_error);
    assert!(
        started
            .text_output
            .contains("Process running with session ID")
    );
    assert_eq!(started.structured_content["output"], "ready");
}
