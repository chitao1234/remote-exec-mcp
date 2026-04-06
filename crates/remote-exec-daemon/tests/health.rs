mod support;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use remote_exec_proto::rpc::TargetInfoResponse;
use reqwest::StatusCode;

#[tokio::test]
async fn target_info_is_available_over_mutual_tls() {
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
        remote_exec_daemon::exec::session::supports_pty_for_mode(
            remote_exec_daemon::config::PtyMode::Auto,
        )
    );
    assert!(info.supports_image_read);
}

#[tokio::test]
async fn daemon_startup_rejects_unusable_default_shell() {
    let err = remote_exec_daemon::run_until(
        remote_exec_daemon::config::DaemonConfig {
            target: "builder-a".to_string(),
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            default_workdir: std::env::temp_dir(),
            sandbox: None,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::Auto,
            default_shell: Some("definitely-not-a-real-shell".to_string()),
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: PathBuf::from("missing-cert.pem"),
                key_pem: PathBuf::from("missing-key.pem"),
                ca_pem: PathBuf::from("missing-ca.pem"),
            },
        },
        std::future::ready(()),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("configured default shell"));
}

#[cfg(not(windows))]
#[tokio::test]
async fn daemon_startup_rejects_non_windows_conpty_configuration() {
    let err = remote_exec_daemon::run_until(
        remote_exec_daemon::config::DaemonConfig {
            target: "builder-a".to_string(),
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            default_workdir: std::env::temp_dir(),
            sandbox: None,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::Conpty,
            default_shell: Some("/bin/sh".to_string()),
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: PathBuf::from("missing-cert.pem"),
                key_pem: PathBuf::from("missing-key.pem"),
                ca_pem: PathBuf::from("missing-ca.pem"),
            },
        },
        std::future::ready(()),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("conpty"));
}
