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

pub fn selected_shell(shell_override: Option<&str>, default_shell: &str) -> String {
    shell_override
        .filter(|value| !value.is_empty())
        .unwrap_or(default_shell)
        .to_string()
}

pub fn shell_argv(shell: &str, login: bool, cmd: &str) -> Vec<String> {
    shell_argv_for_platform(cfg!(windows), shell, login, cmd)
}

fn shell_argv_for_platform(is_windows: bool, shell: &str, login: bool, cmd: &str) -> Vec<String> {
    let lower = shell
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();

    if lower == "powershell.exe" || lower == "powershell" || lower == "pwsh.exe" || lower == "pwsh"
    {
        let mut argv = vec![shell.to_string()];
        if !login {
            argv.push("-NoProfile".to_string());
        }
        argv.push("-Command".to_string());
        argv.push(cmd.to_string());
        return argv;
    }

    if is_windows {
        let mut argv = vec![shell.to_string()];
        if lower == "cmd.exe" || lower == "cmd" {
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
fn usable_unix_shell_candidate(
    candidate: Option<&str>,
    environment: &ProcessEnvironment,
) -> Option<String> {
    usable_unix_shell_candidate_with_validator(
        candidate,
        environment,
        validate_unix_shell_candidate,
    )
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
        "no usable default shell found; tried pwsh.exe, powershell.exe, COMSPEC, and cmd.exe"
    );
}

#[cfg(any(test, windows))]
fn validate_windows_shell_candidate(
    shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    let resolved =
        resolve_windows_command_path(shell, environment).unwrap_or_else(|| shell.to_string());

    probe_shell_for_platform(true, &resolved, environment)
        .with_context(|| format!("failed startup probe for shell `{shell}`"))?;
    Ok(resolved)
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
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use crate::config::ProcessEnvironment;

    use super::resolve_default_unix_shell_with;

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

    fn write_fake_shell(path: &PathBuf, exit_code: i32) {
        std::fs::write(path, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn unix_default_shell_prefers_configured_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        let configured_shell = tempdir.path().join("configured-shell");
        write_fake_shell(&configured_shell, 0);
        let env_shell = tempdir.path().join("env-shell");
        write_fake_shell(&env_shell, 0);
        let environment =
            make_environment(Some(tempdir.path()), Some(&env_shell.to_string_lossy()));

        assert_eq!(
            resolve_default_unix_shell_with(
                Some(&configured_shell.to_string_lossy()),
                environment
                    .var_os("SHELL")
                    .map(|value| value.to_string_lossy().into_owned())
                    .as_deref(),
                &environment,
                || Ok(None),
            )
            .unwrap(),
            configured_shell.to_string_lossy()
        );
    }

    #[test]
    fn unix_default_shell_falls_back_from_unusable_env_shell_to_passwd_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        let env_shell = tempdir.path().join("env-shell");
        write_fake_shell(&env_shell, 1);
        let passwd_shell = tempdir.path().join("passwd-shell");
        write_fake_shell(&passwd_shell, 0);
        let environment =
            make_environment(Some(tempdir.path()), Some(&env_shell.to_string_lossy()));

        assert_eq!(
            resolve_default_unix_shell_with(
                None,
                environment
                    .var_os("SHELL")
                    .map(|value| value.to_string_lossy().into_owned())
                    .as_deref(),
                &environment,
                || Ok(Some(passwd_shell.to_string_lossy().into_owned())),
            )
            .unwrap(),
            passwd_shell.to_string_lossy()
        );
    }

    #[test]
    fn unix_default_shell_uses_bash_from_path_before_bin_sh() {
        let tempdir = tempfile::tempdir().unwrap();
        let bash = tempdir.path().join("bash");
        write_fake_shell(&bash, 0);
        let environment = make_environment(Some(tempdir.path()), None);

        assert_eq!(
            resolve_default_unix_shell_with(None, None, &environment, || Ok(None)).unwrap(),
            bash.to_string_lossy()
        );
    }

    #[test]
    fn unix_default_shell_rejects_unusable_configured_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        let configured_shell = tempdir.path().join("configured-shell");
        write_fake_shell(&configured_shell, 1);
        let environment = make_environment(Some(tempdir.path()), None);

        let err = resolve_default_unix_shell_with(
            Some(&configured_shell.to_string_lossy()),
            None,
            &environment,
            || Ok(None),
        )
        .unwrap_err();

        assert!(err.to_string().contains("configured default shell"));
    }
}

#[cfg(test)]
mod windows_shell_tests {
    use crate::config::ProcessEnvironment;

    use super::{resolve_default_windows_shell_with_validator, shell_argv_for_platform};

    fn make_environment(comspec: Option<&str>) -> ProcessEnvironment {
        let mut environment = ProcessEnvironment::default();
        if let Some(comspec) = comspec {
            environment.set_var("COMSPEC", Some(comspec.into()));
        }
        environment
    }

    #[test]
    fn windows_default_shell_prefers_pwsh_before_legacy_powershell_and_cmd() {
        let environment = make_environment(Some(r"C:\custom\cmd.exe"));

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
    fn windows_default_shell_falls_back_to_comspec() {
        let environment = make_environment(Some(r"C:\custom\cmd.exe"));

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
        let environment = make_environment(None);
        let err =
            resolve_default_windows_shell_with_validator(Some("pwsh.exe"), &environment, |_, _| {
                anyhow::bail!("not usable")
            })
            .unwrap_err();

        assert!(err.to_string().contains("configured default shell"));
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
    }
}
