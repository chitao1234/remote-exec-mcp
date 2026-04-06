use std::ffi::OsStr;
#[cfg(any(test, unix, windows))]
use std::path::Path;
#[cfg(any(test, windows))]
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Context;

use crate::config::ProcessEnvironment;

#[cfg(unix)]
pub fn platform_supports_login_shells() -> bool {
    true
}

#[cfg(windows)]
pub fn platform_supports_login_shells() -> bool {
    true
}

#[cfg(unix)]
pub fn resolve_default_shell(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    let env_shell = environment
        .var_os("SHELL")
        .map(|value| value.to_string_lossy().into_owned());
    resolve_default_unix_shell_with(
        configured_default_shell,
        env_shell.as_deref(),
        environment,
        || -> anyhow::Result<Option<String>> {
            Ok(
                nix::unistd::User::from_uid(nix::unistd::Uid::effective())?.and_then(|user| {
                    let shell = user.shell.to_string_lossy().into_owned();
                    (!shell.is_empty()).then_some(shell)
                }),
            )
        },
    )
}

#[cfg(windows)]
pub fn resolve_default_shell(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    resolve_default_windows_shell_with(configured_default_shell, environment)
}

#[cfg(unix)]
pub fn selected_shell(
    shell_override: Option<&str>,
    default_shell: &str,
    _environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    Ok(shell_override
        .filter(|value| !value.is_empty())
        .unwrap_or(default_shell)
        .to_string())
}

#[cfg(windows)]
pub fn selected_shell(
    shell_override: Option<&str>,
    default_shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    match shell_override.filter(|value| !value.is_empty()) {
        Some(shell) => resolve_requested_windows_shell(shell, environment),
        None => Ok(default_shell.to_string()),
    }
}

pub fn shell_argv(shell: &str, login: bool, cmd: &str) -> Vec<String> {
    shell_argv_for_platform(cfg!(windows), shell, login, cmd)
}

fn shell_argv_for_platform(is_windows: bool, shell: &str, login: bool, cmd: &str) -> Vec<String> {
    let lower = shell_basename_lower(shell);

    if is_windows_powershell_family(&lower) {
        let mut argv = vec![shell.to_string()];
        if !login {
            argv.push("-NoProfile".to_string());
        }
        argv.push("-Command".to_string());
        argv.push(cmd.to_string());
        return argv;
    }

    if is_windows && is_windows_bash_family(&lower) {
        if login {
            return vec![
                shell.to_string(),
                "-l".to_string(),
                "-c".to_string(),
                cmd.to_string(),
            ];
        }
        return vec![shell.to_string(), "-c".to_string(), cmd.to_string()];
    }

    if is_windows {
        let mut argv = vec![shell.to_string()];
        if is_windows_cmd_family(&lower) {
            if !login {
                argv.push("/D".to_string());
            }
            argv.push("/C".to_string());
            argv.push(cmd.to_string());
            return argv;
        }
        argv.push("/C".to_string());
        argv.push(cmd.to_string());
        return argv;
    }

    if login {
        vec![
            shell.to_string(),
            "-l".to_string(),
            "-c".to_string(),
            cmd.to_string(),
        ]
    } else {
        vec![shell.to_string(), "-c".to_string(), cmd.to_string()]
    }
}

#[cfg(unix)]
fn resolve_default_unix_shell_with<F>(
    configured_default_shell: Option<&str>,
    env_shell: Option<&str>,
    environment: &ProcessEnvironment,
    passwd_shell_lookup: F,
) -> anyhow::Result<String>
where
    F: FnOnce() -> anyhow::Result<Option<String>>,
{
    resolve_default_unix_shell_with_validator(
        configured_default_shell,
        env_shell,
        environment,
        passwd_shell_lookup,
        validate_unix_shell_candidate,
    )
}

#[cfg(unix)]
fn resolve_default_unix_shell_with_validator<F, G>(
    configured_default_shell: Option<&str>,
    env_shell: Option<&str>,
    environment: &ProcessEnvironment,
    passwd_shell_lookup: F,
    validate: G,
) -> anyhow::Result<String>
where
    F: FnOnce() -> anyhow::Result<Option<String>>,
    G: Fn(&str, &ProcessEnvironment) -> anyhow::Result<String>,
{
    if let Some(shell) = configured_default_shell.filter(|value| !value.is_empty()) {
        return validate(shell, environment)
            .with_context(|| format!("configured default shell `{shell}` is not usable"));
    }

    if let Some(shell) =
        usable_unix_shell_candidate_with_validator(env_shell, environment, &validate)
    {
        return Ok(shell);
    }

    let passwd_shell = passwd_shell_lookup().ok().flatten();
    if let Some(shell) =
        usable_unix_shell_candidate_with_validator(passwd_shell.as_deref(), environment, &validate)
    {
        return Ok(shell);
    }

    if let Some(shell) =
        usable_unix_shell_candidate_with_validator(Some("bash"), environment, &validate)
    {
        return Ok(shell);
    }
    if let Some(shell) =
        usable_unix_shell_candidate_with_validator(Some("/bin/sh"), environment, &validate)
    {
        return Ok(shell);
    }

    anyhow::bail!("no usable default shell found; tried SHELL, passwd shell, bash, and /bin/sh");
}

#[cfg(unix)]
fn usable_unix_shell_candidate_with_validator<G>(
    candidate: Option<&str>,
    environment: &ProcessEnvironment,
    validate: G,
) -> Option<String>
where
    G: Fn(&str, &ProcessEnvironment) -> anyhow::Result<String>,
{
    let shell = candidate.filter(|value| !value.is_empty())?;
    validate(shell, environment).ok()
}

#[cfg(unix)]
fn validate_unix_shell_candidate(
    shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !is_disallowed_unix_shell(shell),
        "shell `{shell}` is not a usable login shell"
    );

    let resolved = if is_path_like(shell) {
        let path = Path::new(shell);
        anyhow::ensure!(
            shell_path_is_executable(path),
            "shell `{shell}` is not executable"
        );
        shell.to_string()
    } else {
        find_unix_command_on_path(environment.path(), shell)
            .ok_or_else(|| anyhow::anyhow!("shell `{shell}` was not found on PATH"))?
    };

    probe_shell_for_platform(false, &resolved, environment)
        .with_context(|| format!("failed startup probe for shell `{shell}`"))?;
    Ok(resolved)
}

#[cfg(unix)]
fn is_disallowed_unix_shell(shell: &str) -> bool {
    matches!(
        Path::new(shell).file_name().and_then(|name| name.to_str()),
        Some("false" | "nologin")
    )
}

#[cfg(unix)]
fn find_unix_command_on_path(path_env: Option<&OsStr>, command: &str) -> Option<String> {
    std::env::split_paths(path_env?)
        .map(|dir| dir.join(command))
        .find(|path| shell_path_is_executable(path))
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn shell_path_is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    metadata.is_file() && has_execute_bits(&metadata)
}

#[cfg(unix)]
fn has_execute_bits(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(any(test, windows))]
#[cfg_attr(test, allow(dead_code))]
fn resolve_default_windows_shell_with(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    resolve_default_windows_shell_with_validator(
        configured_default_shell,
        environment,
        validate_windows_shell_candidate,
    )
}

#[cfg(any(test, windows))]
fn resolve_default_windows_shell_with_validator<G>(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
    validate: G,
) -> anyhow::Result<String>
where
    G: Fn(&str, &ProcessEnvironment) -> anyhow::Result<String>,
{
    if let Some(shell) = configured_default_shell.filter(|value| !value.is_empty()) {
        return validate(shell, environment)
            .with_context(|| format!("configured default shell `{shell}` is not usable"));
    }

    if let Some(shell) = find_windows_git_bash(environment)
        && let Ok(shell) = validate(&shell, environment)
    {
        return Ok(shell);
    }

    for candidate in ["pwsh.exe", "powershell.exe", "powershell"] {
        if let Ok(shell) = validate(candidate, environment) {
            return Ok(shell);
        }
    }

    if let Some(shell) = environment.comspec().filter(|value| !value.is_empty())
        && let Ok(shell) = validate(shell, environment)
    {
        return Ok(shell);
    }

    if let Ok(shell) = validate("cmd.exe", environment) {
        return Ok(shell);
    }

    anyhow::bail!(
        "no usable default shell found; tried Git Bash, pwsh.exe, powershell.exe, COMSPEC, and cmd.exe"
    );
}

#[cfg(any(test, windows))]
#[cfg_attr(test, allow(dead_code))]
fn validate_windows_shell_candidate(
    shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    let lower = shell_basename_lower(shell);
    let resolved = if !is_path_like(shell) && is_windows_git_bash_alias(&lower) {
        find_windows_git_bash(environment).ok_or_else(|| {
            anyhow::anyhow!(
                "Git Bash was requested via `{shell}` but no Git for Windows bash.exe was found"
            )
        })?
    } else {
        resolve_windows_command_path(shell, environment).unwrap_or_else(|| shell.to_string())
    };

    probe_shell_for_platform(true, &resolved, environment)
        .with_context(|| format!("failed startup probe for shell `{shell}`"))?;
    Ok(resolved)
}

#[cfg(any(test, windows))]
fn resolve_requested_windows_shell(
    shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    let lower = shell_basename_lower(shell);
    if !is_path_like(shell) && is_windows_git_bash_alias(&lower) {
        return find_windows_git_bash(environment).ok_or_else(|| {
            anyhow::anyhow!(
                "Git Bash was requested via `{shell}` but no Git for Windows bash.exe was found"
            )
        });
    }
    Ok(shell.to_string())
}

fn probe_shell_for_platform(
    is_windows: bool,
    shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<()> {
    let argv = shell_argv_for_platform(is_windows, shell, false, "exit 0");
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command.env_clear();
    for (key, value) in environment.vars() {
        command.env(key, value);
    }

    let status = command
        .status()
        .with_context(|| format!("failed to start shell `{shell}`"))?;
    anyhow::ensure!(
        status.success(),
        "startup probe exited with status {status}"
    );
    Ok(())
}

fn is_path_like(command: &str) -> bool {
    let path = Path::new(command);
    path.is_absolute() || path.components().count() > 1
}

fn shell_basename_lower(shell: &str) -> String {
    shell
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase()
}

fn is_windows_powershell_family(lower: &str) -> bool {
    matches!(lower, "powershell.exe" | "powershell" | "pwsh.exe" | "pwsh")
}

fn is_windows_cmd_family(lower: &str) -> bool {
    matches!(lower, "cmd.exe" | "cmd")
}

fn is_windows_bash_family(lower: &str) -> bool {
    matches!(
        lower,
        "bash.exe" | "bash" | "sh.exe" | "sh" | "git-bash.exe" | "git-bash"
    )
}

#[cfg(any(test, windows))]
fn is_windows_git_bash_alias(lower: &str) -> bool {
    matches!(
        lower,
        "bash.exe" | "bash" | "sh.exe" | "sh" | "git-bash.exe" | "git-bash"
    )
}

#[cfg(any(test, windows))]
fn find_windows_git_bash(environment: &ProcessEnvironment) -> Option<String> {
    windows_git_bash_candidates(environment)
        .into_iter()
        .find(|candidate| is_regular_file(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

#[cfg(any(test, windows))]
fn windows_git_bash_candidates(environment: &ProcessEnvironment) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for install_root in windows_git_install_roots(environment) {
        push_git_bash_candidates(&mut candidates, &install_root);
    }

    if let Some(git_path) = resolve_windows_command_path("git.exe", environment)
        .or_else(|| resolve_windows_command_path("git", environment))
    {
        for ancestor in Path::new(&git_path).ancestors().skip(1).take(4) {
            push_git_bash_candidates(&mut candidates, ancestor);
        }
    }

    candidates
}

#[cfg(any(test, windows))]
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
        if !roots.contains(&root) {
            roots.push(root);
        }
    }

    roots
}

#[cfg(any(test, windows))]
fn push_git_bash_candidates(candidates: &mut Vec<PathBuf>, install_root: &Path) {
    for candidate in [
        install_root.join("bin").join("bash.exe"),
        install_root.join("usr").join("bin").join("bash.exe"),
    ] {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
}

#[cfg(any(test, windows))]
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

#[cfg(any(test, windows))]
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

#[cfg(any(test, windows))]
fn windows_path_exts(pathext: Option<&OsStr>) -> Vec<String> {
    let mut exts = pathext
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".com;.exe;.bat;.cmd".to_string())
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(|ext| ext.trim_start_matches('.').to_string())
        .collect::<Vec<_>>();

    if exts.is_empty() {
        exts.extend(["com", "exe", "bat", "cmd"].into_iter().map(str::to_owned));
    }

    exts
}

#[cfg(any(test, windows))]
fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use crate::config::ProcessEnvironment;

    use super::resolve_default_unix_shell_with_validator;

    fn make_environment(path: Option<&std::path::Path>, shell: Option<&str>) -> ProcessEnvironment {
        let mut environment = ProcessEnvironment::default();
        if let Some(path) = path {
            environment.set_var("PATH", Some(OsString::from(path.as_os_str())));
        }
        if let Some(shell) = shell {
            environment.set_var("SHELL", Some(OsString::from(shell)));
        }
        environment
    }

    fn stub_validator(
        resolutions: BTreeMap<String, String>,
    ) -> impl Fn(&str, &ProcessEnvironment) -> anyhow::Result<String> {
        move |shell, _environment| {
            resolutions
                .get(shell)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("shell `{shell}` is not usable"))
        }
    }

    #[test]
    fn unix_default_shell_prefers_configured_shell() {
        let configured_shell = "/opt/test/configured-shell";
        let env_shell = "/opt/test/env-shell";
        let environment =
            make_environment(Some(std::path::Path::new("/opt/test")), Some(env_shell));

        assert_eq!(
            resolve_default_unix_shell_with_validator(
                Some(configured_shell),
                environment
                    .var_os("SHELL")
                    .map(|value| value.to_string_lossy().into_owned())
                    .as_deref(),
                &environment,
                || Ok(None),
                stub_validator(BTreeMap::from([
                    (configured_shell.to_string(), configured_shell.to_string()),
                    (env_shell.to_string(), env_shell.to_string()),
                ])),
            )
            .unwrap(),
            configured_shell
        );
    }

    #[test]
    fn unix_default_shell_falls_back_from_unusable_env_shell_to_passwd_shell() {
        let env_shell = "/opt/test/env-shell";
        let passwd_shell = "/opt/test/passwd-shell";
        let environment =
            make_environment(Some(std::path::Path::new("/opt/test")), Some(env_shell));

        assert_eq!(
            resolve_default_unix_shell_with_validator(
                None,
                environment
                    .var_os("SHELL")
                    .map(|value| value.to_string_lossy().into_owned())
                    .as_deref(),
                &environment,
                || Ok(Some(passwd_shell.to_string())),
                stub_validator(BTreeMap::from([(
                    passwd_shell.to_string(),
                    passwd_shell.to_string(),
                )])),
            )
            .unwrap(),
            passwd_shell
        );
    }

    #[test]
    fn unix_default_shell_uses_bash_from_path_before_bin_sh() {
        let bash = "/opt/test/bash";
        let environment = make_environment(Some(std::path::Path::new("/opt/test")), None);

        assert_eq!(
            resolve_default_unix_shell_with_validator(
                None,
                None,
                &environment,
                || Ok(None),
                stub_validator(BTreeMap::from([("bash".to_string(), bash.to_string())])),
            )
            .unwrap(),
            bash
        );
    }

    #[test]
    fn unix_default_shell_rejects_unusable_configured_shell() {
        let configured_shell = "/opt/test/configured-shell";
        let environment = make_environment(Some(std::path::Path::new("/opt/test")), None);

        let err = resolve_default_unix_shell_with_validator(
            Some(configured_shell),
            None,
            &environment,
            || Ok(None),
            stub_validator(BTreeMap::new()),
        )
        .unwrap_err();

        assert!(err.to_string().contains("configured default shell"));
    }
}

#[cfg(test)]
mod windows_shell_tests {
    use std::path::Path;

    use crate::config::ProcessEnvironment;

    use super::{
        resolve_default_windows_shell_with_validator, resolve_requested_windows_shell,
        shell_argv_for_platform,
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
            resolve_default_windows_shell_with_validator(None, &environment, |candidate, _| {
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
            })
            .unwrap(),
            git_bash.to_string_lossy()
        );
    }

    #[test]
    fn windows_default_shell_falls_back_to_pwsh_before_legacy_powershell_and_cmd() {
        let environment = make_environment(&[], None, Some(r"C:\custom\cmd.exe"));

        assert_eq!(
            resolve_default_windows_shell_with_validator(None, &environment, |candidate, _| {
                match candidate {
                    "pwsh.exe" => Ok(r"C:\tools\pwsh.exe".to_string()),
                    "powershell.exe" => Ok(r"C:\tools\powershell.exe".to_string()),
                    "powershell" => Ok(r"C:\tools\powershell".to_string()),
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    "cmd.exe" => Ok("cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                }
            })
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
            resolve_default_windows_shell_with_validator(None, &environment, |candidate, _| {
                if candidate == git_bash.to_string_lossy() {
                    return Ok(candidate.to_string());
                }
                anyhow::bail!("missing")
            })
            .unwrap(),
            git_bash.to_string_lossy()
        );
    }

    #[test]
    fn windows_default_shell_falls_back_to_comspec() {
        let environment = make_environment(&[], None, Some(r"C:\custom\cmd.exe"));

        assert_eq!(
            resolve_default_windows_shell_with_validator(None, &environment, |candidate, _| {
                match candidate {
                    r"C:\custom\cmd.exe" => Ok(r"C:\custom\cmd.exe".to_string()),
                    _ => anyhow::bail!("missing"),
                }
            })
            .unwrap(),
            r"C:\custom\cmd.exe"
        );
    }

    #[test]
    fn windows_default_shell_rejects_unusable_configured_shell() {
        let environment = make_environment(&[], None, None);
        let err =
            resolve_default_windows_shell_with_validator(Some("pwsh.exe"), &environment, |_, _| {
                anyhow::bail!("not usable")
            })
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
                resolve_requested_windows_shell(alias, &environment).unwrap(),
                git_bash.to_string_lossy(),
                "alias {alias} should resolve to Git Bash"
            );
        }
    }

    #[test]
    fn windows_requested_shell_rejects_bare_bash_when_git_bash_is_missing() {
        let environment = make_environment(&[], None, None);
        let err = resolve_requested_windows_shell("bash.exe", &environment).unwrap_err();

        assert!(err.to_string().contains("Git Bash"));
    }

    #[test]
    fn windows_shell_argv_suppresses_profiles_and_autorun_only_for_non_login_requests() {
        assert_eq!(
            shell_argv_for_platform(true, "pwsh.exe", false, "Write-Output ok"),
            vec![
                "pwsh.exe".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "Write-Output ok".to_string(),
            ]
        );
        assert_eq!(
            shell_argv_for_platform(true, "pwsh.exe", true, "Write-Output ok"),
            vec![
                "pwsh.exe".to_string(),
                "-Command".to_string(),
                "Write-Output ok".to_string(),
            ]
        );
        assert_eq!(
            shell_argv_for_platform(true, "cmd.exe", false, "echo ok"),
            vec![
                "cmd.exe".to_string(),
                "/D".to_string(),
                "/C".to_string(),
                "echo ok".to_string(),
            ]
        );
        assert_eq!(
            shell_argv_for_platform(true, "cmd.exe", true, "echo ok"),
            vec![
                "cmd.exe".to_string(),
                "/C".to_string(),
                "echo ok".to_string(),
            ]
        );
        assert_eq!(
            shell_argv_for_platform(true, "bash.exe", false, "printf ok"),
            vec![
                "bash.exe".to_string(),
                "-c".to_string(),
                "printf ok".to_string(),
            ]
        );
        assert_eq!(
            shell_argv_for_platform(true, "bash.exe", true, "printf ok"),
            vec![
                "bash.exe".to_string(),
                "-l".to_string(),
                "-c".to_string(),
                "printf ok".to_string(),
            ]
        );
    }
}
