mod support;

use std::net::{Ipv4Addr, SocketAddr};

use remote_exec_daemon::config::{
    DaemonConfig, DaemonTransport, ProcessEnvironment, PtyMode, YieldTimeConfig,
};
use remote_exec_proto::rpc::TargetInfoResponse;
use reqwest::StatusCode;

fn startup_validation_config() -> DaemonConfig {
    DaemonConfig {
        target: "builder-a".to_string(),
        listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        default_workdir: std::env::temp_dir(),
        transport: DaemonTransport::Http,
        sandbox: None,
        enable_transfer_compression: true,
        allow_login_shell: true,
        pty: PtyMode::Auto,
        default_shell: None,
        yield_time: YieldTimeConfig::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: ProcessEnvironment::capture_current(),
        tls: None,
    }
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
