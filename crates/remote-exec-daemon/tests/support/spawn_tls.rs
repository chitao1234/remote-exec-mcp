use remote_exec_daemon::config::{DaemonConfig, DaemonTransport, ProcessEnvironment, PtyMode};

use super::super::certs::write_test_certs;
use super::super::fixture::DaemonFixture;

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedClientCert {
    MatchingBrokerLeaf,
    MismatchedDaemonLeaf,
}

fn build_tls_client(certs: &super::super::certs::TestCerts) -> reqwest::Client {
    reqwest::Client::builder()
        .use_rustls_tls()
        .pool_max_idle_per_host(0)
        .add_root_certificate(
            reqwest::Certificate::from_pem(&std::fs::read(&certs.ca_cert).unwrap()).unwrap(),
        )
        .identity(
            reqwest::Identity::from_pem(
                &[
                    std::fs::read(&certs.client_cert).unwrap(),
                    std::fs::read(&certs.client_key).unwrap(),
                ]
                .concat(),
            )
            .unwrap(),
        )
        .build()
        .unwrap()
}

async fn spawn_daemon_with_tls_pty_mode(
    target: &str,
    pty: PtyMode,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    super::install_test_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let (listener, addr) = super::bind_test_listener();
    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = DaemonConfig {
        target: target.to_string(),
        listen: addr,
        default_workdir: workdir.clone(),
        windows_posix_root: None,
        transport: DaemonTransport::Tls,
        http_auth: None,
        sandbox: None,
        enable_transfer_compression: true,
        transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
        max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
        allow_login_shell: true,
        pty,
        default_shell: None,
        yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
        port_forward_limits: remote_exec_daemon::config::HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment,
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: None,
        }),
        request_timeout_ms: 300_000,
    };

    let (shutdown, server_thread) = super::spawn_background_daemon(config, listener);
    let client = build_tls_client(&certs);
    let startup = super::wait_until_ready(
        &client,
        &format!("https://{addr}/v1/health"),
        &server_thread,
    )
    .await;

    if startup == super::StartupWaitOutcome::Ready {
        return super::daemon_fixture(
            tempdir,
            client,
            addr,
            "https",
            workdir,
            shutdown,
            server_thread,
        );
    }

    let err = super::startup_failure_error(
        target,
        addr,
        "health endpoint",
        startup,
        shutdown,
        server_thread,
    );
    panic!("daemon test startup failed: {err:#}");
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_over_tls(target: &str) -> DaemonFixture {
    spawn_daemon_with_tls_pty_mode(target, PtyMode::Auto, ProcessEnvironment::capture_current())
        .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_pinned_client_cert(
    target: &str,
    pinned_client_cert: PinnedClientCert,
) -> DaemonFixture {
    super::install_test_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let (listener, addr) = super::bind_test_listener();
    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let pinned_client_cert_pem = match pinned_client_cert {
        PinnedClientCert::MatchingBrokerLeaf => certs.client_cert.clone(),
        PinnedClientCert::MismatchedDaemonLeaf => certs.daemon_cert.clone(),
    };
    let config = DaemonConfig {
        target: target.to_string(),
        listen: addr,
        default_workdir: workdir.clone(),
        windows_posix_root: None,
        transport: DaemonTransport::Tls,
        http_auth: None,
        sandbox: None,
        enable_transfer_compression: true,
        transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
        max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
        allow_login_shell: true,
        pty: PtyMode::Auto,
        default_shell: None,
        yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
        port_forward_limits: remote_exec_daemon::config::HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: ProcessEnvironment::capture_current(),
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: Some(pinned_client_cert_pem),
        }),
        request_timeout_ms: 300_000,
    };

    let (shutdown, server_thread) = super::spawn_background_daemon(config, listener);
    let client = build_tls_client(&certs);

    let startup = match pinned_client_cert {
        PinnedClientCert::MatchingBrokerLeaf => {
            super::wait_until_ready(
                &client,
                &format!("https://{addr}/v1/health"),
                &server_thread,
            )
            .await
        }
        PinnedClientCert::MismatchedDaemonLeaf => {
            super::wait_until_listener_ready(addr, &server_thread).await
        }
    };

    if startup == super::StartupWaitOutcome::Ready {
        return super::daemon_fixture(
            tempdir,
            client,
            addr,
            "https",
            workdir,
            shutdown,
            server_thread,
        );
    }

    let readiness_target = match pinned_client_cert {
        PinnedClientCert::MatchingBrokerLeaf => "health endpoint",
        PinnedClientCert::MismatchedDaemonLeaf => "listener",
    };
    let err = super::startup_failure_error(
        target,
        addr,
        readiness_target,
        startup,
        shutdown,
        server_thread,
    );
    panic!("daemon test startup failed: {err:#}");
}
