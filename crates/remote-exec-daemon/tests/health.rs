mod support;

use std::net::{Ipv4Addr, SocketAddr};

use remote_exec_daemon::config::{
    DaemonConfig, DaemonTransport, HostPortForwardLimits, ProcessEnvironment, PtyMode,
    YieldTimeConfig,
};
use remote_exec_proto::request_id::{REQUEST_ID_HEADER, RequestId};
use remote_exec_proto::rpc::TargetInfoResponse;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn startup_validation_config() -> DaemonConfig {
    DaemonConfig {
        target: "builder-a".to_string(),
        listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        default_workdir: std::env::temp_dir(),
        windows_posix_root: None,
        transport: DaemonTransport::Http,
        http_auth: None,
        sandbox: None,
        enable_transfer_compression: true,
        transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
        allow_login_shell: true,
        pty: PtyMode::Auto,
        default_shell: None,
        yield_time: YieldTimeConfig::default(),
        port_forward_limits: HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: ProcessEnvironment::capture_current(),
        tls: None,
    }
}

async fn raw_http_request(addr: SocketAddr, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}

#[tokio::test]
async fn daemon_echoes_or_generates_request_id_header() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let echoed = fixture
        .client
        .post(fixture.url("/v1/health"))
        .header(REQUEST_ID_HEADER, "client-req-123")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        echoed
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("client-req-123")
    );

    let generated = fixture
        .client
        .post(fixture.url("/v1/health"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    let request_id = generated
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .expect("request id should be present");
    assert!(RequestId::from_header_value(request_id).is_some());
}

#[tokio::test]
async fn target_info_is_available_over_plain_http() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

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
    assert_eq!(
        info.supports_pty,
        remote_exec_daemon::exec::session::supports_pty_for_mode(PtyMode::Auto)
    );
    assert!(info.supports_image_read);
    assert!(info.supports_transfer_compression);
}

#[tokio::test]
async fn daemon_rejects_http_1_0_rpc_requests() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = raw_http_request(
        fixture.addr,
        "POST /v1/target-info HTTP/1.0\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await;

    if !response.is_empty() {
        assert!(
            response.starts_with("HTTP/1.0 400 Bad Request\r\n")
                || response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{response}"
        );
        assert!(response.contains("\"code\":\"bad_request\""), "{response}");
    }
}

#[tokio::test]
async fn plain_http_bearer_auth_rejects_missing_or_invalid_tokens() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        r#"
[http_auth]
bearer_token = "shared-secret"
"#,
    )
    .await;

    let missing = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer")
    );

    let wrong = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .header(AUTHORIZATION, "Bearer wrong-secret")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn plain_http_bearer_auth_accepts_matching_token() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        r#"
[http_auth]
bearer_token = "shared-secret"
"#,
    )
    .await;

    let info = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .header(AUTHORIZATION, "Bearer shared-secret")
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
async fn daemon_startup_rejects_unusable_default_shell() {
    let mut config = startup_validation_config();
    config.default_shell = Some("definitely-not-a-real-shell".to_string());

    let err = remote_exec_daemon::run_until(config, std::future::ready(()))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("configured default shell"));
}

#[cfg(not(windows))]
#[tokio::test]
async fn daemon_startup_rejects_non_windows_conpty_configuration() {
    let mut config = startup_validation_config();
    config.pty = PtyMode::Conpty;
    config.default_shell = Some("/bin/sh".to_string());

    let err = remote_exec_daemon::run_until(config, std::future::ready(()))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("conpty"));
}
