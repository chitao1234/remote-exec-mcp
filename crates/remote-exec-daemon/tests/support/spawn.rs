use std::net::SocketAddr;
use std::path::Path;

use anyhow::Context;
use remote_exec_daemon::config::{DaemonConfig, DaemonTransport, ProcessEnvironment, PtyMode};

use super::fixture::DaemonFixture;

#[cfg(feature = "tls")]
#[path = "spawn_tls.rs"]
mod spawn_tls;

#[cfg(feature = "tls")]
#[allow(
    unused_imports,
    reason = "Re-exported for TLS-specific integration tests"
)]
pub use spawn_tls::{
    PinnedClientCert, spawn_daemon_over_tls, spawn_daemon_with_pinned_client_cert,
};

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

fn base_daemon_config(
    target: &str,
    listen: SocketAddr,
    default_workdir: &Path,
    pty: PtyMode,
    process_environment: ProcessEnvironment,
) -> DaemonConfig {
    DaemonConfig {
        target: target.to_string(),
        listen,
        default_workdir: default_workdir.to_path_buf(),
        transport: DaemonTransport::Http,
        sandbox: None,
        enable_transfer_compression: true,
        allow_login_shell: true,
        pty,
        default_shell: None,
        yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment,
        tls: None,
    }
}

pub(super) fn install_test_crypto_provider() {
    static INIT: std::sync::Once = std::sync::Once::new();

    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub(super) fn reserve_listen_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

fn daemon_fixture(
    tempdir: tempfile::TempDir,
    client: reqwest::Client,
    addr: SocketAddr,
    scheme: &'static str,
    workdir: std::path::PathBuf,
    shutdown: tokio::sync::oneshot::Sender<()>,
    server_thread: std::thread::JoinHandle<anyhow::Result<()>>,
) -> DaemonFixture {
    DaemonFixture::new(
        tempdir,
        client,
        addr,
        scheme,
        workdir,
        shutdown,
        server_thread,
    )
}

pub(super) async fn wait_until_ready(client: &reqwest::Client, url: &str) {
    for _ in 0..40 {
        if client
            .post(url)
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

#[cfg(feature = "tls")]
pub(super) async fn wait_until_listener_ready(addr: SocketAddr) {
    for _ in 0..40 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    panic!("daemon listener did not become ready");
}

pub(super) fn spawn_background_daemon(
    config: DaemonConfig,
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

async fn spawn_daemon_with_pty_mode(
    target: &str,
    pty: PtyMode,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    install_test_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let addr = reserve_listen_addr();
    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = base_daemon_config(target, addr, &workdir, pty, process_environment);

    let (shutdown, server_thread) = spawn_background_daemon(config);
    let client = reqwest::Client::builder().build().unwrap();
    wait_until_ready(&client, &format!("http://{addr}/v1/health")).await;

    #[cfg(windows)]
    return daemon_fixture(
        tempdir,
        client,
        addr,
        "http",
        workdir,
        shutdown,
        server_thread,
    );

    #[cfg(not(windows))]
    daemon_fixture(
        tempdir,
        client,
        addr,
        "http",
        workdir,
        shutdown,
        server_thread,
    )
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
pub async fn spawn_daemon_over_http(target: &str) -> DaemonFixture {
    spawn_daemon(target).await
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
    install_test_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let addr = reserve_listen_addr();
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
transport = "http"
{extra_config}
"#,
            target = toml_string(target),
            listen = toml_string(&addr.to_string()),
            default_workdir = toml_string(&workdir.display().to_string()),
        ),
    )
    .unwrap();
    let mut config = remote_exec_daemon::config::DaemonConfig::load(&config_path)
        .await
        .unwrap();
    config.process_environment = process_environment;

    let (shutdown, server_thread) = spawn_background_daemon(config);
    let client = reqwest::Client::builder().build().unwrap();
    wait_until_ready(&client, &format!("http://{addr}/v1/health")).await;

    #[cfg(windows)]
    return daemon_fixture(
        tempdir,
        client,
        addr,
        "http",
        workdir,
        shutdown,
        server_thread,
    );

    #[cfg(not(windows))]
    daemon_fixture(
        tempdir,
        client,
        addr,
        "http",
        workdir,
        shutdown,
        server_thread,
    )
}
