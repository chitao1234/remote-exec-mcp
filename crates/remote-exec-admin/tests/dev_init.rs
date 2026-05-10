use std::path::Path;
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

fn assert_failure_contains(output: &Output, expected: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_dev_init(out_dir: &Path, target: &str) -> Output {
    admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(out_dir)
        .args(["--target", target])
        .output()
        .expect("dev-init should run")
}

#[test]
fn dev_init_writes_expected_files_and_config_snippets() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("certs");

    let output = admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&out_dir)
        .args([
            "--target",
            "builder-a",
            "--san",
            "builder-a=dns:builder-a.example.com",
            "--san",
            "builder-a=ip:10.0.0.12",
        ])
        .output()
        .expect("dev-init should run");

    assert_success(&output);

    assert!(out_dir.join("ca.pem").exists());
    assert!(out_dir.join("ca.key").exists());
    assert!(out_dir.join("broker.pem").exists());
    assert!(out_dir.join("broker.key").exists());
    assert!(out_dir.join("certs-manifest.json").exists());
    assert!(out_dir.join("daemons").join("builder-a.pem").exists());
    assert!(out_dir.join("daemons").join("builder-a.key").exists());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("client_cert_pem"));
    assert!(stdout.contains("expected_daemon_name = \"builder-a\""));
    assert!(stdout.contains("cert_pem"));
    assert!(stdout.contains("builder-a.example.com"));
    assert!(stdout.contains("# base_url = \"https://builder-a.example.com:9443\""));
    assert!(stdout.contains("# listen = \"0.0.0.0:9443\""));
    assert!(!stdout.contains("\nbase_url = \"https://builder-a.example.com:9443\""));
    assert!(!stdout.contains("\nlisten = \"0.0.0.0:9443\""));
}

#[test]
fn dev_init_accepts_legacy_daemon_san_alias() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("certs");

    let output = admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&out_dir)
        .args([
            "--target",
            "builder-a",
            "--daemon-san",
            "builder-a=dns:builder-a.example.com",
        ])
        .output()
        .expect("dev-init should run");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("builder-a.example.com"));
}

#[test]
fn dev_init_requires_force_to_overwrite_existing_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("certs");

    let first = run_dev_init(&out_dir, "builder-a");
    assert_success(&first);

    let second = run_dev_init(&out_dir, "builder-a");
    assert_failure_contains(&second, "--force");
}

#[test]
fn dev_init_reuses_ca_from_previous_bundle_directory() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let reused_dir = tempdir.path().join("reused");

    let first = run_dev_init(&source_dir, "builder-a");
    assert_success(&first);

    let second = admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&reused_dir)
        .args(["--target", "builder-b", "--reuse-ca-from-dir"])
        .arg(&source_dir)
        .output()
        .expect("reused dev-init");
    assert_success(&second);

    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.pem")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.pem")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.key")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.key")).unwrap()
    );
    assert!(reused_dir.join("broker.pem").exists());
    assert!(reused_dir.join("daemons").join("builder-b.pem").exists());
}

#[test]
fn dev_init_reuses_ca_from_explicit_pem_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let reused_dir = tempdir.path().join("reused");

    let first = run_dev_init(&source_dir, "builder-a");
    assert_success(&first);

    let second = admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&reused_dir)
        .args(["--target", "builder-b", "--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .args(["--reuse-ca-key-pem"])
        .arg(source_dir.join("ca.key"))
        .output()
        .expect("explicit CA reuse");
    assert_success(&second);

    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.pem")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.pem")).unwrap()
    );
}

#[test]
fn dev_init_rejects_partial_or_mixed_reuse_ca_inputs() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let first = run_dev_init(&source_dir, "builder-a");
    assert_success(&first);

    let partial = admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(tempdir.path().join("partial"))
        .args(["--target", "builder-a", "--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .output()
        .expect("partial reuse");
    assert_failure_contains(&partial, "--reuse-ca-key-pem");

    let mixed = admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(tempdir.path().join("mixed"))
        .args(["--target", "builder-a", "--reuse-ca-from-dir"])
        .arg(&source_dir)
        .args(["--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .args(["--reuse-ca-key-pem"])
        .arg(source_dir.join("ca.key"))
        .output()
        .expect("mixed reuse");
    assert_failure_contains(&mixed, "--reuse-ca-from-dir");
}
