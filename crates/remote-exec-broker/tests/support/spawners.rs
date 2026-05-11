use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::{ServiceExt, transport::TokioChildProcess};
use tempfile::TempDir;

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use super::certs::{TestCerts, write_test_certs_for_daemon_spec};
use super::fixture::{BrokerFixture, DummyClientHandler};
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use super::stub_daemon::spawn_stub_daemon;
use super::stub_daemon::{
    ExecWriteBehavior, StubDaemonState, set_port_forward_support, set_required_bearer_token,
    set_transfer_compression_support, spawn_plain_http_daemon_with_platform,
    spawn_plain_http_retryable_exec_write_daemon, spawn_plain_http_stub_daemon,
    spawn_plain_http_stub_on_listener, spawn_plain_http_unknown_session_exec_write_daemon,
    stub_daemon_state,
};

const TEST_HTTP_READY_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_HTTP_READY_POLL: Duration = Duration::from_millis(50);

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub struct DelayedTargetFixture {
    pub broker: BrokerFixture,
    delayed_listener: tokio::sync::Mutex<Option<tokio::net::TcpListener>>,
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
impl DelayedTargetFixture {
    pub async fn spawn_target(&self, target: &str) {
        let listener = self
            .delayed_listener
            .lock()
            .await
            .take()
            .expect("late target listener should only be consumed once");
        spawn_plain_http_stub_on_listener(
            listener,
            stub_daemon_state(target, ExecWriteBehavior::Success, "linux", true),
        )
        .await;
    }
}

pub struct HttpBrokerFixture {
    pub _tempdir: TempDir,
    pub url: String,
    _stub_state: StubDaemonState,
    _child: tokio::process::Child,
}

pub struct BrokerConfigFixture {
    pub _tempdir: TempDir,
    pub config_path: PathBuf,
    _stub_state: StubDaemonState,
}

struct BrokerConfigTarget<'a> {
    name: &'a str,
    addr: std::net::SocketAddr,
    transport: BrokerTargetTransport<'a>,
    extra_config: Option<&'a str>,
}

#[derive(Clone, Copy)]
enum BrokerTargetTransport<'a> {
    Http,
    #[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
    Https(&'a TestCerts),
    #[cfg(not(all(feature = "broker-tls", feature = "daemon-tls")))]
    _Lifetime(std::marker::PhantomData<&'a ()>),
}

struct LocalBrokerConfig<'a> {
    default_workdir: &'a Path,
    experimental_apply_patch_target_encoding_autodetect: bool,
    extra_config: Option<&'a str>,
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn allocate_unbound_addr_for_dead_target() -> std::net::SocketAddr {
    // Intentionally unbound: these tests need broker startup to see a dead target.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

fn allocate_unbound_addr_for_broker_child() -> std::net::SocketAddr {
    // Temporary child-process fixture handoff. Task 8 replaces this with a
    // broker-owned bind(0) plus bound-address file.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

fn render_broker_target(target: &BrokerConfigTarget<'_>) -> String {
    let extra_config = target
        .extra_config
        .map(|extra| {
            if extra.is_empty() {
                String::new()
            } else {
                format!("{extra}\n")
            }
        })
        .unwrap_or_default();
    match target.transport {
        BrokerTargetTransport::Http => format!(
            r#"[targets.{name}]
base_url = {base_url}
allow_insecure_http = true
expected_daemon_name = {expected_daemon_name}
{extra_config}"#,
            name = target.name,
            base_url = toml_string(&format!("http://{}", target.addr)),
            expected_daemon_name = toml_string(target.name),
            extra_config = extra_config,
        ),
        #[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
        BrokerTargetTransport::Https(certs) => format!(
            r#"[targets.{name}]
base_url = {base_url}
ca_pem = {ca_pem}
client_cert_pem = {client_cert_pem}
client_key_pem = {client_key_pem}
expected_daemon_name = {expected_daemon_name}
{extra_config}"#,
            name = target.name,
            base_url = toml_string(&format!("https://{}", target.addr)),
            ca_pem = toml_string(&certs.ca_cert.display().to_string()),
            client_cert_pem = toml_string(&certs.client_cert.display().to_string()),
            client_key_pem = toml_string(&certs.client_key.display().to_string()),
            expected_daemon_name = toml_string(target.name),
            extra_config = extra_config,
        ),
        #[cfg(not(all(feature = "broker-tls", feature = "daemon-tls")))]
        BrokerTargetTransport::_Lifetime(_) => {
            unreachable!("lifetime marker variant is never constructed")
        }
    }
}

fn render_local_broker_config(local: &LocalBrokerConfig<'_>) -> String {
    let experimental_apply_patch_target_encoding_autodetect =
        if local.experimental_apply_patch_target_encoding_autodetect {
            "experimental_apply_patch_target_encoding_autodetect = true\n"
        } else {
            ""
        };
    let extra_config = local
        .extra_config
        .map(|extra| {
            if extra.is_empty() {
                String::new()
            } else {
                format!("{extra}\n")
            }
        })
        .unwrap_or_default();
    format!(
        r#"[local]
default_workdir = {default_workdir}
{experimental_apply_patch_target_encoding_autodetect}
{extra_config}
"#,
        default_workdir = toml_string(&local.default_workdir.display().to_string()),
        experimental_apply_patch_target_encoding_autodetect =
            experimental_apply_patch_target_encoding_autodetect,
        extra_config = extra_config,
    )
}

fn write_broker_config(
    path: &Path,
    targets: &[BrokerConfigTarget<'_>],
    local: Option<&LocalBrokerConfig<'_>>,
    host_sandbox: Option<&str>,
    extra_top_level: Option<&str>,
) {
    let mut parts = Vec::new();
    if let Some(extra_top_level) = extra_top_level {
        parts.push(extra_top_level.to_string());
    }
    parts.extend(targets.iter().map(render_broker_target));
    if let Some(host_sandbox) = host_sandbox {
        parts.push(host_sandbox.to_string());
    }
    if let Some(local) = local {
        parts.push(render_local_broker_config(local));
    }
    std::fs::write(path, parts.join("\n")).unwrap();
}

fn apply_quiet_test_logging(command: &mut tokio::process::Command, explicit_env: &[(&str, &str)]) {
    if explicit_env
        .iter()
        .any(|(key, _)| *key == "REMOTE_EXEC_LOG" || *key == "RUST_LOG")
    {
        return;
    }
    if std::env::var_os("REMOTE_EXEC_LOG").is_some() || std::env::var_os("RUST_LOG").is_some() {
        return;
    }

    let filter = std::env::var("REMOTE_EXEC_TEST_LOG").unwrap_or_else(|_| "error".to_string());
    command.env("REMOTE_EXEC_LOG", filter);
}

async fn spawn_broker_child(
    config_path: &Path,
) -> rmcp::service::RunningService<rmcp::RoleClient, DummyClientHandler> {
    spawn_broker_child_with_env(config_path, &[]).await
}

async fn spawn_broker_child_with_env(
    config_path: &Path,
    env: &[(&str, &str)],
) -> rmcp::service::RunningService<rmcp::RoleClient, DummyClientHandler> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(config_path);
    apply_quiet_test_logging(&mut command, env);
    command.envs(env.iter().copied());
    let transport = TokioChildProcess::new(command).unwrap();
    DummyClientHandler.serve(transport).await.unwrap()
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub async fn spawn_broker_with_tls_stub_daemon_and_extra_target_config(
    certs: TestCerts,
    extra_target_config: &str,
) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Https(&certs),
            extra_config: Some(extra_target_config),
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;
    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub async fn spawn_broker_with_tls_stub_daemon_and_daemon_spec(
    daemon_spec: remote_exec_pki::DaemonCertSpec,
    extra_target_config: &str,
) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs_for_daemon_spec(tempdir.path(), daemon_spec);
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Https(&certs),
            extra_config: Some(extra_target_config),
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;
    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

pub async fn spawn_broker_with_stub_daemon() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

pub async fn spawn_broker_with_stub_daemon_http_auth(bearer_token: &str) -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut stub_state = stub_daemon_state("builder-a", ExecWriteBehavior::Success, "linux", true);
    set_transfer_compression_support(&mut stub_state, false);
    set_required_bearer_token(&mut stub_state, bearer_token);
    spawn_plain_http_stub_on_listener(listener, stub_state.clone()).await;
    let broker_config = tempdir.path().join("broker.toml");
    let auth_config = format!(
        r#"[targets.builder-a.http_auth]
bearer_token = {}"#,
        toml_string(bearer_token)
    );
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: Some(&auth_config),
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

pub async fn spawn_broker_config_with_stub_daemon() -> BrokerConfigFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    BrokerConfigFixture {
        _tempdir: tempdir,
        config_path: broker_config,
        _stub_state: stub_state,
    }
}

pub async fn spawn_broker_config_local_only() -> BrokerConfigFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    let local_workdir = tempdir.path().join("local-work");
    tokio::fs::create_dir(&local_workdir).await.unwrap();
    write_broker_config(
        &broker_config,
        &[],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: false,
            extra_config: Some("pty = \"none\""),
        }),
        None,
        None,
    );

    BrokerConfigFixture {
        _tempdir: tempdir,
        config_path: broker_config,
        _stub_state: stub_daemon_state("unused", ExecWriteBehavior::Success, "linux", true),
    }
}

pub async fn spawn_broker_local_only() -> BrokerFixture {
    let fixture = spawn_broker_config_local_only().await;
    let client = spawn_broker_child(&fixture.config_path).await;
    BrokerFixture {
        _tempdir: fixture._tempdir,
        client,
        stub_state: fixture._stub_state,
    }
}

pub async fn spawn_broker_local_only_with_port_forward_limit(limit: usize) -> BrokerFixture {
    spawn_broker_local_only_with_port_forward_limits(&format!(
        r#"[port_forward_limits]
max_open_forwards_total = {limit}
"#,
    ))
    .await
}

pub async fn spawn_broker_local_only_with_port_forward_limits(
    extra_top_level: &str,
) -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let local_workdir = tempdir.path().join("local-work");
    std::fs::create_dir_all(&local_workdir).unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: false,
            extra_config: None,
        }),
        None,
        Some(extra_top_level),
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

pub async fn spawn_streamable_http_broker_with_stub_daemon() -> HttpBrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (daemon_addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let broker_addr = allocate_unbound_addr_for_broker_child();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr: daemon_addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        Some(&format!(
            r#"[mcp]
transport = "streamable_http"
listen = {listen}
path = "/mcp"
"#,
            listen = toml_string(&broker_addr.to_string()),
        )),
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    apply_quiet_test_logging(&mut command, &[]);
    command.kill_on_drop(true);
    let child = command.spawn().unwrap();
    let url = format!("http://{broker_addr}/mcp");
    wait_until_ready_mcp_http(&url).await;

    HttpBrokerFixture {
        _tempdir: tempdir,
        url,
        _stub_state: stub_state,
        _child: child,
    }
}

pub async fn spawn_broker_with_stub_daemon_platform(
    platform: &str,
    supports_pty: bool,
) -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) =
        spawn_plain_http_daemon_with_platform(ExecWriteBehavior::Success, platform, supports_pty)
            .await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

pub async fn spawn_broker_with_stub_port_forward_version(version: u32) -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut stub_state = stub_daemon_state("builder-a", ExecWriteBehavior::Success, "linux", true);
    set_port_forward_support(&mut stub_state, true, version);
    spawn_plain_http_stub_on_listener(listener, stub_state.clone()).await;

    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

pub async fn spawn_broker_with_local_and_stub_port_forward_version(version: u32) -> BrokerFixture {
    spawn_broker_with_local_and_stub_port_forward_version_and_extra_config(version, None).await
}

pub async fn spawn_broker_with_local_and_stub_port_forward_version_and_tunnel_queue_limit(
    version: u32,
    max_tunnel_queued_bytes: usize,
) -> BrokerFixture {
    spawn_broker_with_local_and_stub_port_forward_version_and_extra_config(
        version,
        Some(&format!(
            r#"[port_forward_limits]
max_tunnel_queued_bytes = {max_tunnel_queued_bytes}
"#,
        )),
    )
    .await
}

pub async fn spawn_broker_with_local_and_stub_port_forward_version_and_port_forward_limits(
    version: u32,
    extra_top_level: &str,
) -> BrokerFixture {
    spawn_broker_with_local_and_stub_port_forward_version_and_extra_config(
        version,
        Some(extra_top_level),
    )
    .await
}

async fn spawn_broker_with_local_and_stub_port_forward_version_and_extra_config(
    version: u32,
    extra_top_level: Option<&str>,
) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let local_workdir = tempdir.path().join("local-work");
    std::fs::create_dir_all(&local_workdir).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut stub_state = stub_daemon_state("builder-a", ExecWriteBehavior::Success, "linux", true);
    set_port_forward_support(&mut stub_state, true, version);
    spawn_plain_http_stub_on_listener(listener, stub_state.clone()).await;

    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: false,
            extra_config: None,
        }),
        None,
        extra_top_level,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

pub async fn spawn_broker_with_plain_http_stub_daemon() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let mut stub_state =
        stub_daemon_state("builder-xp", ExecWriteBehavior::Success, "windows", false);
    set_transfer_compression_support(&mut stub_state, false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_plain_http_stub_on_listener(listener, stub_state.clone()).await;

    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-xp",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_reverse_ordered_targets() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (live_addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let dead_addr = allocate_unbound_addr_for_dead_target();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[
            BrokerConfigTarget {
                name: "builder-b",
                addr: dead_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: None,
            },
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: None,
            },
        ],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_live_and_dead_targets() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (live_addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let dead_addr = allocate_unbound_addr_for_dead_target();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: None,
            },
            BrokerConfigTarget {
                name: "builder-b",
                addr: dead_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: None,
            },
        ],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_retryable_exec_write_error() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) = spawn_plain_http_retryable_exec_write_daemon().await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_unknown_session_exec_write_error() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) = spawn_plain_http_unknown_session_exec_write_daemon().await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_late_target() -> DelayedTargetFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (live_addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let delayed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let delayed_addr = delayed_listener.local_addr().unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    let late_target_extra_config = r#"[targets.builder-b.timeouts]
startup_probe_ms = 100"#;
    write_broker_config(
        &broker_config,
        &[
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: None,
            },
            BrokerConfigTarget {
                name: "builder-b",
                addr: delayed_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: Some(late_target_extra_config),
            },
        ],
        None,
        None,
        None,
    );

    let client = spawn_broker_child(&broker_config).await;

    DelayedTargetFixture {
        broker: BrokerFixture {
            _tempdir: tempdir,
            client,
            stub_state,
        },
        delayed_listener: tokio::sync::Mutex::new(Some(delayed_listener)),
    }
}

pub async fn spawn_broker_with_local_target() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let local_workdir = tempdir.path().join("local-work");
    std::fs::create_dir_all(&local_workdir).unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: false,
            extra_config: None,
        }),
        None,
        None,
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    apply_quiet_test_logging(&mut command, &[]);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

pub async fn spawn_broker_with_local_target_and_host_sandbox(host_sandbox: &str) -> BrokerFixture {
    spawn_broker_with_local_target_and_host_sandbox_for_workdir(|_| host_sandbox.to_string()).await
}

pub async fn spawn_broker_with_local_target_and_host_sandbox_for_workdir<F>(
    render_host_sandbox: F,
) -> BrokerFixture
where
    F: FnOnce(&Path) -> String,
{
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let local_workdir = tempdir.path().join("local-work");
    std::fs::create_dir_all(&local_workdir).unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    let host_sandbox = render_host_sandbox(&local_workdir);
    write_broker_config(
        &broker_config,
        &[],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: false,
            extra_config: None,
        }),
        Some(&host_sandbox),
        None,
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    apply_quiet_test_logging(&mut command, &[]);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

pub async fn spawn_broker_with_local_target_apply_patch_encoding_autodetect() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let local_workdir = tempdir.path().join("local-work");
    std::fs::create_dir_all(&local_workdir).unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: true,
            extra_config: None,
        }),
        None,
        None,
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    apply_quiet_test_logging(&mut command, &[]);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

pub async fn spawn_broker_with_local_target_and_extra_config(extra_config: &str) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider().unwrap();

    let tempdir = tempfile::tempdir().unwrap();
    let local_workdir = tempdir.path().join("local-work");
    std::fs::create_dir_all(&local_workdir).unwrap();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[],
        Some(&LocalBrokerConfig {
            default_workdir: &local_workdir,
            experimental_apply_patch_target_encoding_autodetect: false,
            extra_config: Some(extra_config),
        }),
        None,
        None,
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    apply_quiet_test_logging(&mut command, &[]);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(TEST_HTTP_READY_TIMEOUT, async {
        loop {
            if client
                .post(format!("http://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(TEST_HTTP_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "plain HTTP stub daemon at http://{addr} did not become ready within {TEST_HTTP_READY_TIMEOUT:?}"
        )
    });
}

async fn wait_until_ready_mcp_http(url: &str) {
    remote_exec_broker::install_crypto_provider().unwrap();
    let client = reqwest::Client::builder().build().unwrap();

    tokio::time::timeout(TEST_HTTP_READY_TIMEOUT, async {
        loop {
            let response = client
                .post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
                .send()
                .await;
            if response
                .as_ref()
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            tokio::time::sleep(TEST_HTTP_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "streamable HTTP broker at {url} did not become ready within {TEST_HTTP_READY_TIMEOUT:?}"
        )
    });
}

pub async fn spawn_broker_with_stub_daemon_and_structured_content_disabled() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            transport: BrokerTargetTransport::Http,
            extra_config: None,
        }],
        None,
        None,
        Some("disable_structured_content = true"),
    );

    let client = spawn_broker_child(&broker_config).await;

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}
