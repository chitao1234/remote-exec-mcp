use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::ProcessEnvironment;
use crate::host_path;

use super::common::{
    is_path_like, is_windows_git_bash_alias, probe_shell_for_platform, shell_basename_lower,
};

#[cfg_attr(test, allow(dead_code))]
pub(super) fn resolve_default_shell(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    resolve_default_windows_shell_with(configured_default_shell, environment, windows_posix_root)
}

#[cfg_attr(test, allow(dead_code))]
pub(super) fn selected_shell(
    shell_override: Option<&str>,
    default_shell: &str,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    match shell_override.filter(|value| !value.is_empty()) {
        Some(shell) => resolve_requested_windows_shell(shell, environment, windows_posix_root),
        None => Ok(default_shell.to_string()),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn resolve_default_windows_shell_with(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    resolve_default_windows_shell_with_validator(
        configured_default_shell,
        environment,
        windows_posix_root,
        validate_windows_shell_candidate,
    )
}

fn resolve_default_windows_shell_with_validator<G>(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
    validate: G,
) -> anyhow::Result<String>
where
    G: Fn(&str, &ProcessEnvironment, Option<&Path>) -> anyhow::Result<String>,
{
    if let Some(shell) = configured_default_shell.filter(|value| !value.is_empty()) {
        return validate(shell, environment, windows_posix_root)
            .with_context(|| format!("configured default shell `{shell}` is not usable"));
    }

    if let Some(shell) = find_windows_bash(environment, windows_posix_root) {
        if let Ok(shell) = validate(&shell, environment, windows_posix_root) {
            return Ok(shell);
        }
    }

    for candidate in ["pwsh.exe", "powershell.exe", "powershell"] {
        if let Ok(shell) = validate(candidate, environment, windows_posix_root) {
            return Ok(shell);
        }
    }

    if let Some(shell) = environment.comspec().filter(|value| !value.is_empty()) {
        if let Ok(shell) = validate(shell, environment, windows_posix_root) {
            return Ok(shell);
        }
    }

    if let Ok(shell) = validate("cmd.exe", environment, windows_posix_root) {
        return Ok(shell);
    }

    anyhow::bail!(
        "no usable default shell found; tried Git Bash, pwsh.exe, powershell.exe, COMSPEC, and cmd.exe"
    );
}

#[cfg_attr(test, allow(dead_code))]
fn validate_windows_shell_candidate(
    shell: &str,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    let shell = resolve_windows_shell_path(shell, windows_posix_root);
    let lower = shell_basename_lower(&shell);
    let resolved = if !is_path_like(&shell) && is_windows_git_bash_alias(&lower) {
        find_windows_bash(environment, windows_posix_root).ok_or_else(|| {
            anyhow::anyhow!(
                "Git Bash was requested via `{shell}` but no Git for Windows bash.exe was found"
            )
        })?
    } else {
        resolve_windows_command_path(&shell, environment).unwrap_or_else(|| shell.clone())
    };

    probe_shell_for_platform(true, &resolved, environment)
        .with_context(|| format!("failed startup probe for shell `{shell}`"))?;
    Ok(resolved)
}

fn resolve_requested_windows_shell(
    shell: &str,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    let shell = resolve_windows_shell_path(shell, windows_posix_root);
    let lower = shell_basename_lower(&shell);
    if !is_path_like(&shell) && is_windows_git_bash_alias(&lower) {
        return find_windows_bash(environment, windows_posix_root).ok_or_else(|| {
            anyhow::anyhow!(
                "Git Bash was requested via `{shell}` but no Git for Windows bash.exe was found"
            )
        });
    }
    Ok(shell)
}

fn find_windows_bash(
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> Option<String> {
    windows_bash_candidates(environment, windows_posix_root)
        .into_iter()
        .find(|candidate| is_regular_file(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

fn windows_bash_candidates(
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(root) = windows_posix_root {
        push_bash_candidates(&mut candidates, root);
    }

    for install_root in windows_git_install_roots(environment) {
        push_bash_candidates(&mut candidates, &install_root);
    }

    if let Some(git_path) = resolve_windows_command_path("git.exe", environment)
        .or_else(|| resolve_windows_command_path("git", environment))
    {
        for ancestor in Path::new(&git_path).ancestors().skip(1).take(4) {
            push_bash_candidates(&mut candidates, ancestor);
        }
    }

    candidates
}

fn windows_git_install_roots(environment: &ProcessEnvironment) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    for (key, suffix) in [
        ("ProgramFiles", PathBuf::from("Git")),
        ("ProgramFiles(x86)", PathBuf::from("Git")),
        ("ProgramW6432", PathBuf::from("Git")),
        ("LocalAppData", PathBuf::from(r"Programs\Git")),
    ] {
        let Some(base) = environment.var_os(key).map(PathBuf::from) else {
            continue;
        };
        let root = base.join(&suffix);
        push_unique_path(&mut roots, root);
    }

    roots
}

fn push_bash_candidates(candidates: &mut Vec<PathBuf>, install_root: &Path) {
    for candidate in [
        install_root.join("bin").join("bash.exe"),
        install_root.join("usr").join("bin").join("bash.exe"),
    ] {
        push_unique_path(candidates, candidate);
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths
        .iter()
        .any(|existing| same_windows_candidate_path(existing, &path))
    {
        paths.push(path);
    }
}

fn same_windows_candidate_path(left: &Path, right: &Path) -> bool {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    remote_exec_proto::path::same_path_for_policy(
        remote_exec_proto::path::windows_path_policy(),
        left.as_ref(),
        right.as_ref(),
    )
}

fn resolve_windows_shell_path(shell: &str, windows_posix_root: Option<&Path>) -> String {
    host_path::resolve_absolute_input_path(shell, windows_posix_root)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| shell.to_string())
}

fn resolve_windows_command_path(command: &str, environment: &ProcessEnvironment) -> Option<String> {
    if is_path_like(command) {
        return windows_path_candidates(Path::new(command), environment.var_os("PATHEXT"))
            .into_iter()
            .find(|candidate| is_regular_file(candidate))
            .map(|candidate| candidate.to_string_lossy().into_owned());
    }

    std::env::split_paths(environment.path()?)
        .flat_map(|dir| {
            windows_path_candidates(&dir.join(command), environment.var_os("PATHEXT")).into_iter()
        })
        .find(|candidate| is_regular_file(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

fn windows_path_candidates(path: &Path, pathext: Option<&OsStr>) -> Vec<PathBuf> {
    if path.extension().is_some() {
        return vec![path.to_path_buf()];
    }

    let mut candidates = vec![path.to_path_buf()];
    candidates.extend(
        windows_path_exts(pathext)
            .into_iter()
            .map(|ext| path.with_extension(ext)),
    );
    candidates
}

fn windows_path_exts(pathext: Option<&OsStr>) -> Vec<String> {
    let mut exts = Vec::new();

    if let Some(pathext) = pathext {
        for ext in pathext.to_string_lossy().split(';') {
            push_windows_path_ext(&mut exts, ext);
        }
    } else {
        push_default_windows_path_exts(&mut exts);
    }

    if exts.is_empty() {
        push_default_windows_path_exts(&mut exts);
    }

    exts
}

fn push_windows_path_ext(exts: &mut Vec<String>, ext: &str) {
    let ext = ext.trim().trim_start_matches('.');
    if !ext.is_empty()
        && !exts
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(ext))
    {
        exts.push(ext.to_string());
    }
}

fn push_default_windows_path_exts(exts: &mut Vec<String>) {
    for ext in ["com", "exe", "bat", "cmd"] {
        push_windows_path_ext(exts, ext);
    }
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::config::ProcessEnvironment;

    use super::{
        push_unique_path, resolve_default_windows_shell_with_validator,
        resolve_requested_windows_shell, windows_path_exts,
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
                |candidate, _, _| match candidate {
                    candidate if candidate == git_bash.to_string_lossy() => {
                        Ok(candidate.to_string())
                    }
                    "pwsh.exe" => Ok(r"C:\tools\pwsh.exe".to_string()),
                    "powershell.exe" => Ok(r"C:\tools\powershell.exe".to_string()),
                    "powershell" => Ok(r"C:\tools\powershell".to_string()),
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    "cmd.exe" => Ok("cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                },
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
                |candidate, _, _| match candidate {
                    "pwsh.exe" => Ok(r"C:\tools\pwsh.exe".to_string()),
                    "powershell.exe" => Ok(r"C:\tools\powershell.exe".to_string()),
                    "powershell" => Ok(r"C:\tools\powershell".to_string()),
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    "cmd.exe" => Ok("cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                },
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
                },
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
                |candidate, _, _| match candidate {
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                },
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

    #[test]
    fn windows_path_candidate_deduping_ignores_case_and_separator_style() {
        let mut paths = vec![std::path::PathBuf::from(r"C:\Git\bin\bash.exe")];
        push_unique_path(&mut paths, std::path::PathBuf::from("c:/git/bin/bash.exe"));
        assert_eq!(paths.len(), 1);
    }
}
