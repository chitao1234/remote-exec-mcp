use std::path::Path;

use crate::config::ProcessEnvironment;

use super::{
    resolve_default_windows_shell_with_validator, resolve_requested_windows_shell,
    windows_path_exts,
};

fn make_environment(
    path_entries: &[&Path],
    program_files: Option<&Path>,
    comspec: Option<&str>,
) -> ProcessEnvironment {
    let mut environment = ProcessEnvironment::default();
    if !path_entries.is_empty() {
        environment.set_var("PATH", Some(std::env::join_paths(path_entries).unwrap()));
    }
    if let Some(program_files) = program_files {
        environment.set_var(
            "ProgramFiles",
            Some(program_files.as_os_str().to_os_string()),
        );
    }
    if let Some(comspec) = comspec {
        environment.set_var("COMSPEC", Some(comspec.into()));
    }
    environment.set_var("PATHEXT", Some(".EXE".into()));
    environment
}

#[test]
fn windows_default_shell_prefers_git_bash_before_pwsh_and_cmd() {
    let tempdir = tempfile::tempdir().unwrap();
    let git_bash = tempdir.path().join("Git").join("bin").join("bash.exe");
    std::fs::create_dir_all(git_bash.parent().unwrap()).unwrap();
    std::fs::write(&git_bash, b"stub").unwrap();
    let environment = make_environment(&[], Some(tempdir.path()), Some(r"C:\custom\cmd.exe"));

    assert_eq!(
        resolve_default_windows_shell_with_validator(
            None,
            &environment,
            None,
            |candidate, _, _| {
                match candidate {
                    candidate if candidate == git_bash.to_string_lossy() => {
                        Ok(candidate.to_string())
                    }
                    "pwsh.exe" => Ok(r"C:\tools\pwsh.exe".to_string()),
                    "powershell.exe" => Ok(r"C:\tools\powershell.exe".to_string()),
                    "powershell" => Ok(r"C:\tools\powershell".to_string()),
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    "cmd.exe" => Ok("cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                }
            }
        )
        .unwrap(),
        git_bash.to_string_lossy()
    );
}

#[test]
fn windows_default_shell_falls_back_to_pwsh_before_legacy_powershell_and_cmd() {
    let environment = make_environment(&[], None, Some(r"C:\custom\cmd.exe"));

    assert_eq!(
        resolve_default_windows_shell_with_validator(
            None,
            &environment,
            None,
            |candidate, _, _| {
                match candidate {
                    "pwsh.exe" => Ok(r"C:\tools\pwsh.exe".to_string()),
                    "powershell.exe" => Ok(r"C:\tools\powershell.exe".to_string()),
                    "powershell" => Ok(r"C:\tools\powershell".to_string()),
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    "cmd.exe" => Ok("cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                }
            }
        )
        .unwrap(),
        r"C:\tools\pwsh.exe"
    );
}

#[test]
fn windows_default_shell_can_derive_git_bash_from_git_exe_on_path() {
    let tempdir = tempfile::tempdir().unwrap();
    let git_cmd = tempdir.path().join("Git").join("cmd");
    let git_exe = git_cmd.join("git.exe");
    let git_bash = tempdir
        .path()
        .join("Git")
        .join("usr")
        .join("bin")
        .join("bash.exe");
    std::fs::create_dir_all(&git_cmd).unwrap();
    std::fs::create_dir_all(git_bash.parent().unwrap()).unwrap();
    std::fs::write(&git_exe, b"stub").unwrap();
    std::fs::write(&git_bash, b"stub").unwrap();
    let environment = make_environment(&[&git_cmd], None, Some(r"C:\custom\cmd.exe"));

    assert_eq!(
        resolve_default_windows_shell_with_validator(
            None,
            &environment,
            None,
            |candidate: &str, _, _| {
                if candidate == git_bash.to_string_lossy() {
                    return Ok(candidate.to_string());
                }
                anyhow::bail!("missing")
            }
        )
        .unwrap(),
        git_bash.to_string_lossy()
    );
}

#[test]
fn windows_default_shell_falls_back_to_comspec() {
    let environment = make_environment(&[], None, Some(r"C:\custom\cmd.exe"));

    assert_eq!(
        resolve_default_windows_shell_with_validator(
            None,
            &environment,
            None,
            |candidate, _, _| {
                match candidate {
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                }
            }
        )
        .unwrap(),
        r"C:\custom\cmd.exe"
    );
}

#[test]
fn windows_default_shell_rejects_unusable_configured_shell() {
    let environment = make_environment(&[], None, None);
    let err = resolve_default_windows_shell_with_validator(
        Some("pwsh.exe"),
        &environment,
        None,
        |_, _, _| anyhow::bail!("not usable"),
    )
    .unwrap_err();

    assert!(err.to_string().contains("configured default shell"));
}

#[test]
fn windows_requested_shell_resolves_git_bash_aliases_to_git_bash_path() {
    let tempdir = tempfile::tempdir().unwrap();
    let git_bash = tempdir.path().join("Git").join("bin").join("bash.exe");
    std::fs::create_dir_all(git_bash.parent().unwrap()).unwrap();
    std::fs::write(&git_bash, b"stub").unwrap();
    let environment = make_environment(&[], Some(tempdir.path()), None);

    for alias in [
        "bash.exe",
        "bash",
        "sh.exe",
        "sh",
        "git-bash.exe",
        "git-bash",
    ] {
        assert_eq!(
            resolve_requested_windows_shell(alias, &environment, None).unwrap(),
            git_bash.to_string_lossy(),
            "alias {alias} should resolve to Git Bash"
        );
    }
}

#[test]
fn windows_requested_shell_rejects_bare_bash_when_git_bash_is_missing() {
    let environment = make_environment(&[], None, None);
    let err = resolve_requested_windows_shell("bash.exe", &environment, None).unwrap_err();

    assert!(err.to_string().contains("Git Bash"));
}

#[test]
fn windows_path_exts_deduplicates_and_falls_back_for_empty_values() {
    assert_eq!(
        windows_path_exts(Some(std::ffi::OsStr::new(".EXE;.exe; .CMD ;"))),
        vec!["EXE".to_string(), "CMD".to_string()]
    );
    assert_eq!(
        windows_path_exts(Some(std::ffi::OsStr::new(";;"))),
        vec![
            "com".to_string(),
            "exe".to_string(),
            "bat".to_string(),
            "cmd".to_string(),
        ]
    );
}

#[cfg(windows)]
#[test]
fn windows_path_candidate_deduping_ignores_case_and_separator_style() {
    let mut paths = vec![std::path::PathBuf::from(r"C:\Git\bin\bash.exe")];
    super::push_unique_path(&mut paths, std::path::PathBuf::from("c:/git/bin/bash.exe"));
    assert_eq!(paths.len(), 1);
}
