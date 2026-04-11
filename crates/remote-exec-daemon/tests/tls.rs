#![cfg(feature = "tls")]

mod support;

use remote_exec_proto::rpc::TargetInfoResponse;

#[tokio::test]
async fn target_info_is_available_over_mutual_tls() {
    let fixture = support::spawn::spawn_daemon_over_tls("builder-a").await;

    let info = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json::<TargetInfoResponse>()
        .await
        .unwrap();

    assert_eq!(info.target, "builder-a");
}

#[tokio::test]
async fn target_info_is_available_with_matching_pinned_client_certificate() {
    let fixture = support::spawn::spawn_daemon_with_pinned_client_cert(
        "builder-a",
        support::spawn::PinnedClientCert::MatchingBrokerLeaf,
    )
    .await;

    let info = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json::<TargetInfoResponse>()
        .await
        .unwrap();

    assert_eq!(info.target, "builder-a");
}

#[tokio::test]
async fn daemon_rejects_mismatched_pinned_client_certificate() {
    let fixture = support::spawn::spawn_daemon_with_pinned_client_cert(
        "builder-a",
        support::spawn::PinnedClientCert::MismatchedDaemonLeaf,
    )
    .await;

    let err = fixture
        .client
        .post(fixture.url("/v1/health"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap_err();

    assert!(err.is_request(), "unexpected error: {err:#}");
}
