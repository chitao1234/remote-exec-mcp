use std::process::Command;

fn apply_patch() -> Command {
    Command::new(env!("CARGO_BIN_EXE_apply_patch"))
}

#[test]
fn prints_builtin_help() {
    let output = apply_patch().arg("--help").output().unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Apply a Codex-style patch")
    );
}

#[test]
fn prints_help_from_file() {
    let tempdir = tempfile::tempdir().unwrap();
    let help_path = tempdir.path().join("help.txt");
    std::fs::write(&help_path, "Custom help text\n").unwrap();

    let output = apply_patch()
        .arg("--help")
        .arg("--help-file")
        .arg(help_path)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Custom help text\n"
    );
}

#[test]
fn help_file_without_help_is_ignored() {
    let tempdir = tempfile::tempdir().unwrap();
    let missing_help_path = tempdir.path().join("missing-help.txt");
    let patch = "*** Begin Patch\n*** Add File: created.txt\n+hello\n*** End Patch\n";

    let output = apply_patch()
        .current_dir(tempdir.path())
        .arg("--help-file")
        .arg(missing_help_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;

            child.stdin.take().unwrap().write_all(patch.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("created.txt")).unwrap(),
        "hello\n"
    );
}

#[test]
fn names_the_failed_action_for_preflight_errors() {
    let tempdir = tempfile::tempdir().unwrap();
    let patch = "*** Begin Patch\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch\n";

    let output = apply_patch()
        .current_dir(tempdir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;

            child.stdin.take().unwrap().write_all(patch.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("failed to update `missing.txt`:")
    );
}

#[cfg(unix)]
#[test]
fn reports_partial_success_after_a_runtime_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tempdir = tempfile::tempdir().unwrap();
    let locked_dir = tempdir.path().join("locked");
    std::fs::create_dir(&locked_dir).unwrap();
    std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let patch = "*** Begin Patch\n*** Add File: created.txt\n+created\n*** Add File: locked/blocked.txt\n+blocked\n*** End Patch\n";

    let output = apply_patch()
        .current_dir(tempdir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;

            child.stdin.take().unwrap().write_all(patch.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();

    std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Partial success. Updated the following files:\nA created.txt\n"
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("failed to add `locked/blocked.txt`: Permission denied")
    );
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("created.txt")).unwrap(),
        "created\n"
    );
}
