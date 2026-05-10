use std::process::{Command, Output};

fn admin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_ca(out_dir: &std::path::Path) {
    let output = admin()
        .args(["certs", "init-ca", "--out-dir"])
        .arg(out_dir)
        .output()
        .expect("init-ca runs");
    assert_success(&output);
}

#[test]
fn init_ca_writes_only_ca_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("ca");

    init_ca(&out_dir);
    assert!(out_dir.join("ca.pem").exists());
    assert!(out_dir.join("ca.key").exists());
    assert!(!out_dir.join("broker.pem").exists());
    assert!(!out_dir.join("certs-manifest.json").exists());
}

#[test]
fn issue_broker_uses_existing_ca_and_writes_only_broker_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let ca_dir = tempdir.path().join("ca");
    let broker_dir = tempdir.path().join("broker");

    init_ca(&ca_dir);

    let output = admin()
        .args(["certs", "issue-broker", "--ca-cert-pem"])
        .arg(ca_dir.join("ca.pem"))
        .args(["--ca-key-pem"])
        .arg(ca_dir.join("ca.key"))
        .args(["--out-dir"])
        .arg(&broker_dir)
        .output()
        .expect("issue-broker runs");

    assert_success(&output);
    assert!(broker_dir.join("broker.pem").exists());
    assert!(broker_dir.join("broker.key").exists());
    assert!(!broker_dir.join("certs-manifest.json").exists());
}

#[test]
fn issue_daemon_writes_target_named_leaf_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let ca_dir = tempdir.path().join("ca");
    let daemon_dir = tempdir.path().join("daemon");

    init_ca(&ca_dir);

    let output = admin()
        .args(["certs", "issue-daemon", "--ca-cert-pem"])
        .arg(ca_dir.join("ca.pem"))
        .args(["--ca-key-pem"])
        .arg(ca_dir.join("ca.key"))
        .args(["--out-dir"])
        .arg(&daemon_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("issue-daemon runs");

    assert_success(&output);
    assert!(daemon_dir.join("builder-a.pem").exists());
    assert!(daemon_dir.join("builder-a.key").exists());
    assert!(!daemon_dir.join("certs-manifest.json").exists());
}
