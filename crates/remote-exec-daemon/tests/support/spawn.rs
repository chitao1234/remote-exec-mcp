use std::net::SocketAddr;

use remote_exec_daemon::config::ProcessEnvironment;
#[cfg(windows)]
use remote_exec_daemon::config::WindowsPtyBackendOverride;

use super::certs::write_test_certs;
use super::fixture::DaemonFixture;

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPtyTestBackend {
    Conpty,
    Winpty,
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
impl WindowsPtyTestBackend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Conpty => "conpty",
            Self::Winpty => "winpty",
        }
    }

    fn backend_override(self) -> WindowsPtyBackendOverride {
        match self {
            Self::Conpty => WindowsPtyBackendOverride::PortablePty,
            Self::Winpty => WindowsPtyBackendOverride::Winpty,
        }
    }
}

async fn spawn_daemon_with_backend_override(
    target: &str,
    windows_pty_backend_override: Option<remote_exec_daemon::config::WindowsPtyBackendOverride>,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = remote_exec_daemon::config::DaemonConfig {
        target: target.to_string(),
        listen: addr,
        default_workdir: workdir.clone(),
        allow_login_shell: true,
        windows_pty_backend_override,
        process_environment,
        tls: remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
        },
    };

    tokio::spawn(remote_exec_daemon::run(config));

    let client = reqwest::Client::builder()
        .use_rustls_tls()
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
        .unwrap();

    wait_until_ready(&client, addr).await;
    DaemonFixture {
        _tempdir: tempdir,
        client,
        addr,
        workdir,
    }
}

pub async fn spawn_daemon(target: &str) -> DaemonFixture {
    spawn_daemon_with_backend_override(target, None, ProcessEnvironment::capture_current()).await
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub fn supported_windows_pty_backends() -> Vec<WindowsPtyTestBackend> {
    let mut backends = Vec::new();

    if remote_exec_daemon::exec::session::supports_pty_with_override(Some(
        WindowsPtyBackendOverride::PortablePty,
    )) {
        backends.push(WindowsPtyTestBackend::Conpty);
    }
    if remote_exec_daemon::exec::session::supports_pty_with_override(Some(
        WindowsPtyBackendOverride::Winpty,
    )) {
        backends.push(WindowsPtyTestBackend::Winpty);
    }

    assert!(
        !backends.is_empty(),
        "expected at least one Windows PTY backend to be available"
    );
    backends
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_for_windows_pty_backend(
    target: &str,
    backend: WindowsPtyTestBackend,
) -> DaemonFixture {
    spawn_daemon_with_backend_override(
        target,
        Some(backend.backend_override()),
        ProcessEnvironment::capture_current(),
    )
    .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_process_environment(
    target: &str,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    spawn_daemon_with_backend_override(target, None, process_environment).await
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_for_windows_pty_backend_with_process_environment(
    target: &str,
    backend: WindowsPtyTestBackend,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    spawn_daemon_with_backend_override(
        target,
        Some(backend.backend_override()),
        process_environment,
    )
    .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_extra_config(target: &str, extra_config: &str) -> DaemonFixture {
    spawn_daemon_with_extra_config_and_process_environment(
        target,
        extra_config,
        ProcessEnvironment::capture_current(),
    )
    .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_extra_config_and_process_environment(
    target: &str,
    extra_config: &str,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config_path = tempdir.path().join("daemon.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"target = {target}
listen = {listen}
default_workdir = {default_workdir}
{extra_config}

[tls]
cert_pem = {cert_pem}
key_pem = {key_pem}
ca_pem = {ca_pem}
"#,
            target = toml_string(target),
            listen = toml_string(&addr.to_string()),
            default_workdir = toml_string(&workdir.display().to_string()),
            cert_pem = toml_string(&certs.daemon_cert.display().to_string()),
            key_pem = toml_string(&certs.daemon_key.display().to_string()),
            ca_pem = toml_string(&certs.ca_cert.display().to_string()),
        ),
    )
    .unwrap();
    let mut config = remote_exec_daemon::config::DaemonConfig::load(&config_path)
        .await
        .unwrap();
    config.process_environment = process_environment;

    tokio::spawn(remote_exec_daemon::run(config));

    let client = reqwest::Client::builder()
        .use_rustls_tls()
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
        .unwrap();

    wait_until_ready(&client, addr).await;
    DaemonFixture {
        _tempdir: tempdir,
        client,
        addr,
        workdir,
    }
}

async fn wait_until_ready(client: &reqwest::Client, addr: SocketAddr) {
    for _ in 0..40 {
        if client
            .post(format!("https://{addr}/v1/health"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("daemon did not become ready");
}
