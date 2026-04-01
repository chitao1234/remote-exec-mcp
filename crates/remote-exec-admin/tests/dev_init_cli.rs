use std::process::Command;

#[test]
fn dev_init_help_lists_required_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--help"])
        .output()
        .expect("admin help should run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("certs"));
    assert!(stdout.contains("dev-init"));
    assert!(stdout.contains("--out-dir"));
    assert!(stdout.contains("--target"));
    assert!(stdout.contains("--daemon-san"));
    assert!(stdout.contains("--broker-common-name"));
    assert!(stdout.contains("--force"));
}
