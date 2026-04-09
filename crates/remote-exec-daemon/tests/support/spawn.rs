use std::net::SocketAddr;
use std::path::Path;
#[cfg(windows)]
use std::sync::OnceLock;

use anyhow::Context;
use remote_exec_daemon::config::{DaemonTransport, ProcessEnvironment, PtyMode};

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

    fn pty_mode(self) -> PtyMode {
        match self {
            Self::Conpty => PtyMode::Conpty,
            Self::Winpty => PtyMode::Winpty,
        }
    }
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedClientCert {
    MatchingBrokerLeaf,
    MismatchedDaemonLeaf,
}

async fn spawn_daemon_with_pty_mode(
    target: &str,
    pty: PtyMode,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    #[cfg(windows)]
    let concurrency_guard = daemon_test_lock().lock().await;

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
        transport: DaemonTransport::Tls,
        sandbox: None,
        enable_transfer_compression: true,
        allow_login_shell: true,
        pty,
        default_shell: None,
        process_environment,
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: None,
        }),
    };

    let (shutdown, server_thread) = spawn_background_daemon(config);
    let client = build_client(&certs);

    wait_until_ready(&client, addr).await;
    #[cfg(windows)]
    return DaemonFixture::new(
        tempdir,
        client,
        addr,
        "https",
        workdir,
        shutdown,
        server_thread,
        concurrency_guard,
    );

    #[cfg(not(windows))]
    DaemonFixture::new(
        tempdir,
        client,
        addr,
        "https",
        workdir,
        shutdown,
        server_thread,
    )
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_pinned_client_cert(
    target: &str,
    pinned_client_cert: PinnedClientCert,
) -> DaemonFixture {
    #[cfg(windows)]
    let concurrency_guard = daemon_test_lock().lock().await;

    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let pinned_client_cert_pem = match pinned_client_cert {
        PinnedClientCert::MatchingBrokerLeaf => certs.client_cert.clone(),
        PinnedClientCert::MismatchedDaemonLeaf => certs.daemon_cert.clone(),
    };
    let config = remote_exec_daemon::config::DaemonConfig {
        target: target.to_string(),
        listen: addr,
        default_workdir: workdir.clone(),
        transport: DaemonTransport::Tls,
        sandbox: None,
        enable_transfer_compression: true,
        allow_login_shell: true,
        pty: PtyMode::Auto,
        default_shell: None,
        process_environment: ProcessEnvironment::capture_current(),
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: Some(pinned_client_cert_pem),
        }),
    };

    let (shutdown, server_thread) = spawn_background_daemon(config);
    let client = build_client(&certs);

    match pinned_client_cert {
        PinnedClientCert::MatchingBrokerLeaf => wait_until_ready(&client, addr).await,
        PinnedClientCert::MismatchedDaemonLeaf => wait_until_listener_ready(addr).await,
    }
    #[cfg(windows)]
    return DaemonFixture::new(
        tempdir,
        client,
        addr,
        "https",
        workdir,
        shutdown,
        server_thread,
        concurrency_guard,
    );

    #[cfg(not(windows))]
    DaemonFixture::new(
        tempdir,
        client,
        addr,
        "https",
        workdir,
        shutdown,
        server_thread,
    )
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_over_http(target: &str) -> DaemonFixture {
    #[cfg(windows)]
    let concurrency_guard = daemon_test_lock().lock().await;

    let tempdir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = remote_exec_daemon::config::DaemonConfig {
        target: target.to_string(),
        listen: addr,
        default_workdir: workdir.clone(),
        transport: DaemonTransport::Http,
        sandbox: None,
        enable_transfer_compression: true,
        allow_login_shell: true,
        pty: PtyMode::Auto,
        default_shell: None,
        process_environment: ProcessEnvironment::capture_current(),
        tls: None,
    };

    let (shutdown, server_thread) = spawn_background_daemon(config);
    let client = reqwest::Client::builder().build().unwrap();

    wait_until_ready_http(&client, addr).await;
    #[cfg(windows)]
    return DaemonFixture::new(
        tempdir,
        client,
        addr,
        "http",
        workdir,
        shutdown,
        server_thread,
        concurrency_guard,
    );

    #[cfg(not(windows))]
    DaemonFixture::new(
        tempdir,
        client,
        addr,
        "http",
        workdir,
        shutdown,
        server_thread,
    )
}

fn build_client(certs: &super::certs::TestCerts) -> reqwest::Client {
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

fn spawn_background_daemon(
    config: remote_exec_daemon::config::DaemonConfig,
) -> (
    tokio::sync::oneshot::Sender<()>,
    std::thread::JoinHandle<anyhow::Result<()>>,
) {
    let target = config.target.clone();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server_thread = std::thread::Builder::new()
        .name(format!("remote-exec-daemon-test-{target}"))
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to build daemon test runtime")?;
            runtime.block_on(remote_exec_daemon::run_until(config, async move {
                let _ = shutdown_rx.await;
            }))
        })
        .unwrap();

    (shutdown_tx, server_thread)
}

#[cfg(windows)]
fn daemon_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    // Windows daemon integration tests exercise real PTY backends and are not
    // isolated enough to run multiple daemon fixtures concurrently in one test
    // process. Hold this guard for the full fixture lifetime.
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon(target: &str) -> DaemonFixture {
    spawn_daemon_with_pty_mode(target, PtyMode::Auto, ProcessEnvironment::capture_current()).await
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub fn supported_windows_pty_backends() -> Vec<WindowsPtyTestBackend> {
    let mut backends = Vec::new();

    if remote_exec_daemon::exec::session::supports_pty_for_mode(PtyMode::Conpty) {
        backends.push(WindowsPtyTestBackend::Conpty);
    }
    if remote_exec_daemon::exec::session::supports_pty_for_mode(PtyMode::Winpty) {
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
    spawn_daemon_with_pty_mode(
        target,
        backend.pty_mode(),
        ProcessEnvironment::capture_current(),
    )
    .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_process_environment(
    target: &str,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    spawn_daemon_with_pty_mode(target, PtyMode::Auto, process_environment).await
}

#[cfg(windows)]
#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_for_windows_pty_backend_with_process_environment(
    target: &str,
    backend: WindowsPtyTestBackend,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    spawn_daemon_with_pty_mode(target, backend.pty_mode(), process_environment).await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_extra_config(target: &str, extra_config: &str) -> DaemonFixture {
    spawn_daemon_with_extra_config_for_workdir_and_process_environment(
        target,
        |_| extra_config.to_string(),
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
    spawn_daemon_with_extra_config_for_workdir_and_process_environment(
        target,
        |_| extra_config.to_string(),
        process_environment,
    )
    .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_extra_config_for_workdir<F>(
    target: &str,
    render_extra_config: F,
) -> DaemonFixture
where
    F: FnOnce(&Path) -> String,
{
    spawn_daemon_with_extra_config_for_workdir_and_process_environment(
        target,
        render_extra_config,
        ProcessEnvironment::capture_current(),
    )
    .await
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_with_extra_config_for_workdir_and_process_environment<F>(
    target: &str,
    render_extra_config: F,
    process_environment: ProcessEnvironment,
) -> DaemonFixture
where
    F: FnOnce(&Path) -> String,
{
    #[cfg(windows)]
    let concurrency_guard = daemon_test_lock().lock().await;

    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let extra_config = render_extra_config(&workdir);
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

    let (shutdown, server_thread) = spawn_background_daemon(config);
    let client = build_client(&certs);

    wait_until_ready(&client, addr).await;
    #[cfg(windows)]
    return DaemonFixture::new(
        tempdir,
        client,
        addr,
        "https",
        workdir,
        shutdown,
        server_thread,
        concurrency_guard,
    );

    #[cfg(not(windows))]
    DaemonFixture::new(
        tempdir,
        client,
        addr,
        "https",
        workdir,
        shutdown,
        server_thread,
    )
}

async fn wait_until_ready(client: &reqwest::Client, addr: SocketAddr) {
    for _ in 0..40 {
        if client
            .post(format!("https://{addr}/v1/health"))
            .header(reqwest::header::CONNECTION, "close")
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

async fn wait_until_ready_http(client: &reqwest::Client, addr: SocketAddr) {
    for _ in 0..40 {
        if client
            .post(format!("http://{addr}/v1/health"))
            .header(reqwest::header::CONNECTION, "close")
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

async fn wait_until_listener_ready(addr: SocketAddr) {
    for _ in 0..40 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("daemon listener did not become ready");
}
