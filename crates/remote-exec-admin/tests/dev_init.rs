use std::process::Command;

#[test]
fn dev_init_writes_expected_files_and_config_snippets() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("certs");

    let output = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&out_dir)
        .args([
            "--target",
            "builder-a",
            "--daemon-san",
            "builder-a=dns:builder-a.example.com",
            "--daemon-san",
            "builder-a=ip:10.0.0.12",
        ])
        .output()
        .expect("dev-init should run");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

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
}

#[test]
fn dev_init_requires_force_to_overwrite_existing_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("certs");

    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&out_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init should run");
    assert!(first.status.success());

    let second = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&out_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("overwrite check should run");
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("--force"),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn dev_init_reuses_ca_from_previous_bundle_directory() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let reused_dir = tempdir.path().join("reused");

    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&source_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&reused_dir)
        .args(["--target", "builder-b", "--reuse-ca-from-dir"])
        .arg(&source_dir)
        .output()
        .expect("reused dev-init");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

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

    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&source_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&reused_dir)
        .args(["--target", "builder-b", "--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .args(["--reuse-ca-key-pem"])
        .arg(source_dir.join("ca.key"))
        .output()
        .expect("explicit CA reuse");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.pem")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.pem")).unwrap()
    );
}

#[test]
fn dev_init_rejects_partial_or_mixed_reuse_ca_inputs() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&source_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let partial = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(tempdir.path().join("partial"))
        .args(["--target", "builder-a", "--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .output()
        .expect("partial reuse");
    assert!(!partial.status.success());
    assert!(String::from_utf8_lossy(&partial.stderr).contains("--reuse-ca-key-pem"));

    let mixed = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
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
    assert!(!mixed.status.success());
    assert!(String::from_utf8_lossy(&mixed.stderr).contains("--reuse-ca-from-dir"));
}
