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
fn applies_patch_from_standard_input() {
    let tempdir = tempfile::tempdir().unwrap();
    let patch = "*** Begin Patch\n*** Add File: created.txt\n+hello\n*** End Patch\n";

    let output = apply_patch()
        .current_dir(tempdir.path())
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
