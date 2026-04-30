use std::path::{Path, PathBuf};

use axum::serve;
use rmcp::{ServiceExt, transport::TokioChildProcess};
use tempfile::TempDir;

use super::certs::allocate_addr;
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use super::certs::{TestCerts, write_test_certs_for_daemon_spec};
use super::fixture::{BrokerFixture, DummyClientHandler};
#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
use super::stub_daemon::spawn_stub_daemon;
use super::stub_daemon::{
    ExecWriteBehavior, StubDaemonState, set_required_bearer_token,
    set_transfer_compression_support, spawn_named_plain_http_daemon_on_addr,
    spawn_plain_http_daemon_with_platform, spawn_plain_http_retryable_exec_write_daemon,
    spawn_plain_http_stub_daemon, spawn_plain_http_unknown_session_exec_write_daemon,
    stub_daemon_state, stub_router,
};

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub struct DelayedTargetFixture {
    pub broker: BrokerFixture,
    addr: std::net::SocketAddr,
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
impl DelayedTargetFixture {
    pub async fn spawn_target(&self, target: &str) {
        spawn_named_plain_http_daemon_on_addr(
            self.addr,
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

async fn spawn_broker_child(
    config_path: &Path,
) -> rmcp::service::RunningService<rmcp::RoleClient, DummyClientHandler> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(config_path);
    let transport = TokioChildProcess::new(command).unwrap();
    DummyClientHandler.serve(transport).await.unwrap()
}

#[cfg(all(feature = "broker-tls", feature = "daemon-tls"))]
pub async fn spawn_broker_with_tls_stub_daemon_and_extra_target_config(
    certs: TestCerts,
    extra_target_config: &str,
) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

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
    remote_exec_daemon::install_crypto_provider();

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
    let app = stub_router(stub_state.clone());
    tokio::spawn(async move {
        serve(listener, app).await.unwrap();
    });
    wait_until_ready_http(addr).await;
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

pub async fn spawn_streamable_http_broker_with_stub_daemon() -> HttpBrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let (daemon_addr, stub_state) = spawn_plain_http_stub_daemon().await;
    let broker_addr = allocate_addr();
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

pub async fn spawn_broker_with_plain_http_stub_daemon() -> BrokerFixture {
    let tempdir = tempfile::tempdir().unwrap();
    let mut stub_state =
        stub_daemon_state("builder-xp", ExecWriteBehavior::Success, "windows", false);
    set_transfer_compression_support(&mut stub_state, false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = stub_router(stub_state.clone());

    tokio::spawn(async move {
        serve(listener, app).await.unwrap();
    });

    wait_until_ready_http(addr).await;

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
    let dead_addr = allocate_addr();
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
    let dead_addr = allocate_addr();
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
    let delayed_addr = allocate_addr();
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
                addr: delayed_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: None,
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
        addr: delayed_addr,
    }
}

pub async fn spawn_broker_with_local_target() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

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
    remote_exec_daemon::install_crypto_provider();

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
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

pub async fn spawn_broker_with_local_target_apply_patch_encoding_autodetect() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

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
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

pub async fn spawn_broker_with_local_target_and_extra_config(extra_config: &str) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

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
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: stub_daemon_state("local", ExecWriteBehavior::Success, "local", true),
    }
}

async fn wait_until_ready_http(addr: std::net::SocketAddr) {
    remote_exec_broker::install_crypto_provider();
    let client = reqwest::Client::builder().build().unwrap();

    for _ in 0..40 {
        if client
            .post(format!("http://{addr}/v1/health"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    panic!("plain http stub daemon did not become ready");
}

async fn wait_until_ready_mcp_http(url: &str) {
    remote_exec_broker::install_crypto_provider();
    let client = reqwest::Client::builder().build().unwrap();

    for _ in 0..40 {
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    panic!("streamable HTTP broker did not become ready");
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
