mod support;

use remote_exec_proto::rpc::TargetInfoResponse;
use reqwest::StatusCode;

#[tokio::test]
async fn target_info_is_available_over_mutual_tls() {
    let fixture = support::spawn_daemon("builder-a").await;

    let health = fixture
        .client
        .post(fixture.url("/v1/health"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

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
    assert_eq!(
        info.hostname,
        gethostname::gethostname().to_string_lossy().into_owned()
    );
    assert_eq!(info.platform, std::env::consts::OS);
    assert_eq!(info.arch, std::env::consts::ARCH);
    assert_eq!(info.supports_pty, remote_exec_daemon::exec::session::supports_pty());
    assert!(info.supports_image_read);
}
