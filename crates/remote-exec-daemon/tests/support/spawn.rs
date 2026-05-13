use std::any::Any;
use std::net::SocketAddr;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

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
        windows_posix_root: None,
        transport: DaemonTransport::Http,
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
        tls: None,
    }
}

pub(super) fn install_test_crypto_provider() {
    static INIT: std::sync::Once = std::sync::Once::new();

    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub(super) fn bind_test_listener() -> (std::net::TcpListener, SocketAddr) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    (listener, addr)
}

const STARTUP_POLL_ATTEMPTS: usize = 100;
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartupWaitOutcome {
    Ready,
    ThreadFinished,
    TimedOut,
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

pub(super) async fn wait_until_ready(
    client: &reqwest::Client,
    url: &str,
    server_thread: &JoinHandle<anyhow::Result<()>>,
) -> StartupWaitOutcome {
    for _ in 0..STARTUP_POLL_ATTEMPTS {
        if client
            .post(url)
            .header(reqwest::header::CONNECTION, "close")
            .json(&serde_json::json!({}))
            .send()
            .await
            .is_ok()
        {
            return StartupWaitOutcome::Ready;
        }
        if server_thread.is_finished() {
            return StartupWaitOutcome::ThreadFinished;
        }
        tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
    }

    if server_thread.is_finished() {
        StartupWaitOutcome::ThreadFinished
    } else {
        StartupWaitOutcome::TimedOut
    }
}

#[cfg(feature = "tls")]
pub(super) async fn wait_until_listener_ready(
    addr: SocketAddr,
    server_thread: &JoinHandle<anyhow::Result<()>>,
) -> StartupWaitOutcome {
    for _ in 0..STARTUP_POLL_ATTEMPTS {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return StartupWaitOutcome::Ready;
        }
        if server_thread.is_finished() {
            return StartupWaitOutcome::ThreadFinished;
        }
        tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
    }

    if server_thread.is_finished() {
        StartupWaitOutcome::ThreadFinished
    } else {
        StartupWaitOutcome::TimedOut
    }
}

pub(super) fn spawn_background_daemon(
    config: DaemonConfig,
    listener: std::net::TcpListener,
) -> (
    tokio::sync::oneshot::Sender<()>,
    JoinHandle<anyhow::Result<()>>,
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
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)
                    .context("failed to adopt daemon test listener")?;
                remote_exec_daemon::run_until_on_listener(config, listener, async move {
                    let _ = shutdown_rx.await;
                })
                .await
            })
        })
        .unwrap();

    (shutdown_tx, server_thread)
}

pub(super) fn join_server_thread(
    server_thread: JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    match server_thread.join() {
        Ok(result) => result,
        Err(payload) => anyhow::bail!(
            "daemon test thread panicked: {}",
            panic_payload_message(payload)
        ),
    }
}

pub(super) fn startup_failure_error(
    target: &str,
    addr: SocketAddr,
    readiness_target: &str,
    outcome: StartupWaitOutcome,
    shutdown: tokio::sync::oneshot::Sender<()>,
    server_thread: JoinHandle<anyhow::Result<()>>,
) -> anyhow::Error {
    match outcome {
        StartupWaitOutcome::Ready => anyhow::anyhow!(
            "internal error: startup_failure_error called after successful startup for target `{target}` on {addr}"
        ),
        StartupWaitOutcome::ThreadFinished => match join_server_thread(server_thread) {
            Ok(()) => anyhow::anyhow!(
                "daemon test thread exited before {readiness_target} became ready for target `{target}` on {addr}"
            ),
            Err(err) => err.context(format!(
                "daemon test startup failed before {readiness_target} became ready for target `{target}` on {addr}"
            )),
        },
        StartupWaitOutcome::TimedOut => {
            let _ = shutdown.send(());
            match join_server_thread(server_thread) {
                Ok(()) => anyhow::anyhow!(
                    "daemon did not become ready at {readiness_target} for target `{target}` on {addr} within {} ms",
                    STARTUP_POLL_ATTEMPTS as u64 * STARTUP_POLL_INTERVAL.as_millis() as u64
                ),
                Err(err) => err.context(format!(
                    "daemon test startup timed out waiting for {readiness_target} for target `{target}` on {addr}"
                )),
            }
        }
    }
}

fn panic_payload_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "non-string panic payload".to_string(),
        },
    }
}

async fn spawn_daemon_with_pty_mode(
    target: &str,
    pty: PtyMode,
    process_environment: ProcessEnvironment,
) -> DaemonFixture {
    install_test_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let (listener, addr) = bind_test_listener();
    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = base_daemon_config(target, addr, &workdir, pty, process_environment);

    let (shutdown, server_thread) = spawn_background_daemon(config, listener);
    let client = reqwest::Client::builder().build().unwrap();
    let startup =
        wait_until_ready(&client, &format!("http://{addr}/v1/health"), &server_thread).await;

    if startup == StartupWaitOutcome::Ready {
        return daemon_fixture(
            tempdir,
            client,
            addr,
            "http",
            workdir,
            shutdown,
            server_thread,
        );
    }

    let err = startup_failure_error(
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
    F: Fn(&Path) -> String,
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
    F: Fn(&Path) -> String,
{
    install_test_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let (listener, addr) = bind_test_listener();
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

    let (shutdown, server_thread) = spawn_background_daemon(config, listener);
    let client = reqwest::Client::builder().build().unwrap();
    let startup =
        wait_until_ready(&client, &format!("http://{addr}/v1/health"), &server_thread).await;

    if startup == StartupWaitOutcome::Ready {
        return daemon_fixture(
            tempdir,
            client,
            addr,
            "http",
            workdir,
            shutdown,
            server_thread,
        );
    }

    let err = startup_failure_error(
        target,
        addr,
        "health endpoint",
        startup,
        shutdown,
        server_thread,
    );
    panic!("daemon test startup failed: {err:#}");
}
