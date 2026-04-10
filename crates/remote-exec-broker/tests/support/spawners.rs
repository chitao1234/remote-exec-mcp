use std::path::{Path, PathBuf};

use axum::serve;
use rmcp::{ServiceExt, transport::TokioChildProcess};
use tempfile::TempDir;

use super::certs::{TestCerts, allocate_addr, write_test_certs, write_test_certs_for_daemon_spec};
use super::fixture::{BrokerFixture, DummyClientHandler};
use super::stub_daemon::{
    ExecWriteBehavior, StubDaemonState, set_transfer_compression_support,
    spawn_daemon_with_platform, spawn_named_daemon_on_addr, spawn_retryable_exec_write_daemon,
    spawn_stub_daemon, spawn_unknown_session_exec_write_daemon, stub_daemon_state, stub_router,
};

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub struct DelayedTargetFixture {
    pub broker: BrokerFixture,
    certs: TestCerts,
    addr: std::net::SocketAddr,
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
impl DelayedTargetFixture {
    pub async fn spawn_target(&self, target: &str) {
        spawn_named_daemon_on_addr(
            &self.certs,
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
    certs: &'a TestCerts,
    extra_config: Option<&'a str>,
}

struct LocalBrokerConfig<'a> {
    default_workdir: &'a Path,
    experimental_apply_patch_target_encoding_autodetect: bool,
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
    format!(
        r#"[targets.{name}]
base_url = {base_url}
ca_pem = {ca_pem}
client_cert_pem = {client_cert_pem}
client_key_pem = {client_key_pem}
expected_daemon_name = {expected_daemon_name}
{extra_config}"#,
        name = target.name,
        base_url = toml_string(&format!("https://{}", target.addr)),
        ca_pem = toml_string(&target.certs.ca_cert.display().to_string()),
        client_cert_pem = toml_string(&target.certs.client_cert.display().to_string()),
        client_key_pem = toml_string(&target.certs.client_key.display().to_string()),
        expected_daemon_name = toml_string(target.name),
        extra_config = extra_config,
    )
}

fn render_insecure_http_broker_target(name: &str, addr: std::net::SocketAddr) -> String {
    format!(
        r#"[targets.{name}]
base_url = {base_url}
allow_insecure_http = true
expected_daemon_name = {expected_daemon_name}
"#,
        name = name,
        base_url = toml_string(&format!("http://{addr}")),
        expected_daemon_name = toml_string(name),
    )
}

fn render_local_broker_config(local: &LocalBrokerConfig<'_>) -> String {
    let experimental_apply_patch_target_encoding_autodetect =
        if local.experimental_apply_patch_target_encoding_autodetect {
            "experimental_apply_patch_target_encoding_autodetect = true\n"
        } else {
            ""
        };
    format!(
        r#"[local]
default_workdir = {default_workdir}
{experimental_apply_patch_target_encoding_autodetect}
"#,
        default_workdir = toml_string(&local.default_workdir.display().to_string()),
        experimental_apply_patch_target_encoding_autodetect =
            experimental_apply_patch_target_encoding_autodetect,
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

pub async fn spawn_broker_with_stub_daemon_and_extra_target_config(
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
            certs: &certs,
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

pub async fn spawn_broker_with_stub_daemon_and_daemon_spec(
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
            certs: &certs,
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
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            certs: &certs,
            extra_config: None,
        }],
        None,
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
        stub_state,
    }
}

pub async fn spawn_broker_config_with_stub_daemon() -> BrokerConfigFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            certs: &certs,
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

pub async fn spawn_streamable_http_broker_with_stub_daemon() -> HttpBrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (daemon_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr: daemon_addr,
            certs: &certs,
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
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) =
        spawn_daemon_with_platform(&certs, ExecWriteBehavior::Success, platform, supports_pty)
            .await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            certs: &certs,
            extra_config: None,
        }],
        None,
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
    std::fs::write(
        &broker_config,
        render_insecure_http_broker_target("builder-xp", addr),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_reverse_ordered_targets() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let dead_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[
            BrokerConfigTarget {
                name: "builder-b",
                addr: dead_addr,
                certs: &certs,
                extra_config: None,
            },
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                certs: &certs,
                extra_config: None,
            },
        ],
        None,
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
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_live_and_dead_targets() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let dead_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                certs: &certs,
                extra_config: None,
            },
            BrokerConfigTarget {
                name: "builder-b",
                addr: dead_addr,
                certs: &certs,
                extra_config: None,
            },
        ],
        None,
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
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_retryable_exec_write_error() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_retryable_exec_write_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            certs: &certs,
            extra_config: None,
        }],
        None,
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
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_unknown_session_exec_write_error() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_unknown_session_exec_write_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            certs: &certs,
            extra_config: None,
        }],
        None,
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
        stub_state,
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
pub async fn spawn_broker_with_late_target() -> DelayedTargetFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let delayed_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                certs: &certs,
                extra_config: None,
            },
            BrokerConfigTarget {
                name: "builder-b",
                addr: delayed_addr,
                certs: &certs,
                extra_config: None,
            },
        ],
        None,
        None,
        None,
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    DelayedTargetFixture {
        broker: BrokerFixture {
            _tempdir: tempdir,
            client,
            stub_state,
        },
        certs,
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
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    write_broker_config(
        &broker_config,
        &[BrokerConfigTarget {
            name: "builder-a",
            addr,
            certs: &certs,
            extra_config: None,
        }],
        None,
        None,
        Some("disable_structured_content = true"),
    );

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}
