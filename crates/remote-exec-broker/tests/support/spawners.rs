#![allow(dead_code)]

use std::path::Path;

use rmcp::{ServiceExt, transport::TokioChildProcess};

use super::certs::{TestCerts, allocate_addr, write_test_certs};
use super::fixture::{BrokerFixture, DummyClientHandler};
use super::stub_daemon::{
    ExecWriteBehavior, spawn_daemon_with_platform, spawn_named_daemon_on_addr,
    spawn_retryable_exec_write_daemon, spawn_stub_daemon, spawn_unknown_session_exec_write_daemon,
    stub_daemon_state,
};

pub struct DelayedTargetFixture {
    pub broker: BrokerFixture,
    certs: TestCerts,
    addr: std::net::SocketAddr,
}

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

struct BrokerConfigTarget<'a> {
    name: &'a str,
    addr: std::net::SocketAddr,
    certs: &'a TestCerts,
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn render_broker_target(target: &BrokerConfigTarget<'_>) -> String {
    format!(
        r#"[targets.{name}]
base_url = {base_url}
ca_pem = {ca_pem}
client_cert_pem = {client_cert_pem}
client_key_pem = {client_key_pem}
expected_daemon_name = {expected_daemon_name}
"#,
        name = target.name,
        base_url = toml_string(&format!("https://{}", target.addr)),
        ca_pem = toml_string(&target.certs.ca_cert.display().to_string()),
        client_cert_pem = toml_string(&target.certs.client_cert.display().to_string()),
        client_key_pem = toml_string(&target.certs.client_key.display().to_string()),
        expected_daemon_name = toml_string(target.name),
    )
}

fn write_broker_config(path: &Path, targets: &[BrokerConfigTarget<'_>]) {
    let config = targets
        .iter()
        .map(render_broker_target)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, config).unwrap();
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
        }],
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
        }],
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
            },
            BrokerConfigTarget {
                name: "builder-a",
                addr: live_addr,
                certs: &certs,
            },
        ],
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
            },
            BrokerConfigTarget {
                name: "builder-b",
                addr: dead_addr,
                certs: &certs,
            },
        ],
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
        }],
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
        }],
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
            },
            BrokerConfigTarget {
                name: "builder-b",
                addr: delayed_addr,
                certs: &certs,
            },
        ],
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
