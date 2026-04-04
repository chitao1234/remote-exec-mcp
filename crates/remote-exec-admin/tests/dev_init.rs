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
