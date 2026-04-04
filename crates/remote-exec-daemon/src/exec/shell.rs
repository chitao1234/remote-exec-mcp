use std::ffi::OsStr;
#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
pub fn platform_supports_login_shells() -> bool {
    true
}

#[cfg(windows)]
pub fn platform_supports_login_shells() -> bool {
    true
}

#[cfg(unix)]
pub fn resolve_shell(shell_override: Option<&str>) -> anyhow::Result<String> {
    let env_shell = std::env::var("SHELL").ok();
    Ok(resolve_shell_with(
        shell_override,
        env_shell.as_deref(),
        || -> anyhow::Result<Option<String>> {
            Ok(
                nix::unistd::User::from_uid(nix::unistd::Uid::effective())?.and_then(|user| {
                    let shell = user.shell.to_string_lossy().into_owned();
                    (!shell.is_empty()).then_some(shell)
                }),
            )
        },
    ))
}

#[cfg(windows)]
pub fn resolve_shell(shell_override: Option<&str>) -> anyhow::Result<String> {
    let path = std::env::var_os("PATH");
    let comspec = std::env::var("COMSPEC").ok();
    Ok(resolve_windows_shell_with(
        shell_override,
        path.as_deref(),
        comspec.as_deref(),
    ))
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

#[cfg(any(test, windows))]
fn find_first_on_path(path_env: Option<&OsStr>, names: &[&str]) -> Option<String> {
    std::env::split_paths(path_env?)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| {
            std::fs::metadata(path)
                .map(|meta| meta.is_file())
                .unwrap_or(false)
        })
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(any(test, windows))]
fn resolve_windows_shell_with(
    shell_override: Option<&str>,
    path_env: Option<&OsStr>,
    comspec: Option<&str>,
) -> String {
    if let Some(shell) = shell_override.filter(|value| !value.is_empty()) {
        return shell.to_string();
    }
    if let Some(shell) = find_first_on_path(path_env, &["pwsh.exe"]) {
        return shell;
    }
    if let Some(shell) = find_first_on_path(path_env, &["powershell.exe", "powershell"]) {
        return shell;
    }
    if let Some(shell) = comspec.filter(|value| !value.is_empty()) {
        return shell.to_string();
    }
    "cmd.exe".to_string()
}

#[cfg(unix)]
fn choose_shell(
    shell_override: Option<&str>,
    env_shell: Option<&str>,
    passwd_shell: Option<&str>,
) -> String {
    preferred_shell(shell_override, env_shell, passwd_shell).unwrap_or("/bin/sh".to_string())
}

#[cfg(unix)]
fn preferred_shell(
    shell_override: Option<&str>,
    env_shell: Option<&str>,
    passwd_shell: Option<&str>,
) -> Option<String> {
    shell_override
        .filter(|value| !value.is_empty())
        .or_else(|| env_shell.filter(|value| !value.is_empty()))
        .or_else(|| usable_passwd_shell(passwd_shell))
        .map(str::to_owned)
}

#[cfg(unix)]
fn resolve_shell_with<F>(
    shell_override: Option<&str>,
    env_shell: Option<&str>,
    passwd_shell_lookup: F,
) -> String
where
    F: FnOnce() -> anyhow::Result<Option<String>>,
{
    let passwd_shell = passwd_shell_lookup().ok().flatten();
    let path = std::env::var_os("PATH");
    let fallback = choose_shell(shell_override, env_shell, passwd_shell.as_deref());

    preferred_shell(shell_override, env_shell, passwd_shell.as_deref())
        .or_else(|| find_bash_in_path(path.as_deref()))
        .unwrap_or(fallback)
}

#[cfg(unix)]
fn find_bash_in_path(path_env: Option<&OsStr>) -> Option<String> {
    std::env::split_paths(path_env?)
        .map(|dir| dir.join("bash"))
        .find(|path| shell_path_is_executable(path))
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn usable_passwd_shell(passwd_shell: Option<&str>) -> Option<&str> {
    passwd_shell.filter(|value| {
        !value.is_empty()
            && !matches!(
                Path::new(value).file_name().and_then(|name| name.to_str()),
                Some("false" | "nologin")
            )
            && shell_path_is_executable(Path::new(value))
    })
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

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::{choose_shell, resolve_shell_with};

    static PATH_LOCK: Mutex<()> = Mutex::new(());

    struct PathGuard {
        original: Option<OsString>,
    }

    impl PathGuard {
        fn set(path: Option<&OsStr>) -> Self {
            let original = std::env::var_os("PATH");
            unsafe {
                match path {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
            Self { original }
        }
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.original {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    fn with_path_var<T>(path: Option<&OsStr>, test: impl FnOnce() -> T) -> T {
        let _lock = PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = PathGuard::set(path);
        test()
    }

    fn make_executable_shell() -> (tempfile::TempDir, PathBuf) {
        let tempdir = tempfile::tempdir().unwrap();
        let shell_path = tempdir.path().join("shell");
        std::fs::write(&shell_path, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&shell_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&shell_path, permissions).unwrap();
        (tempdir, shell_path)
    }

    #[test]
    fn choose_shell_prefers_explicit_override() {
        assert_eq!(
            choose_shell(Some("/bin/zsh"), Some("/bin/sh"), Some("/bin/bash")),
            "/bin/zsh"
        );
    }

    #[test]
    fn choose_shell_uses_env_before_passwd_and_fallback() {
        let (_tempdir, shell_path) = make_executable_shell();
        let shell_text = shell_path.to_string_lossy().into_owned();
        assert_eq!(
            choose_shell(None, Some("/bin/sh"), Some(&shell_text)),
            "/bin/sh"
        );
        assert_eq!(choose_shell(None, None, Some(&shell_text)), shell_text);
        assert_eq!(choose_shell(None, None, None), "/bin/sh");
    }

    #[test]
    fn choose_shell_ignores_unusable_passwd_shells() {
        assert_eq!(
            choose_shell(None, Some("/bin/sh"), Some("/usr/sbin/nologin")),
            "/bin/sh"
        );
        assert_eq!(choose_shell(None, None, Some("/bin/false")), "/bin/sh");
    }

    #[test]
    fn resolve_shell_ignores_passwd_lookup_failure() {
        with_path_var(None, || {
            assert_eq!(
                resolve_shell_with(None, Some("/bin/sh"), || {
                    Err(anyhow::anyhow!("passwd lookup failed"))
                }),
                "/bin/sh"
            );
            assert_eq!(
                resolve_shell_with(None, None, || Err(anyhow::anyhow!("passwd lookup failed"))),
                "/bin/sh"
            );
        });
    }

    #[test]
    fn resolve_shell_ignores_missing_passwd_shell() {
        with_path_var(None, || {
            assert_eq!(
                resolve_shell_with(None, None, || Ok(Some("/opt/missing-shell".to_string()))),
                "/bin/sh"
            );
        });
    }

    #[test]
    fn resolve_shell_ignores_non_executable_passwd_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        let shell_path = tempdir.path().join("shell");
        std::fs::write(&shell_path, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&shell_path).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&shell_path, permissions).unwrap();

        with_path_var(None, || {
            assert_eq!(
                resolve_shell_with(None, None, || {
                    Ok(Some(shell_path.to_string_lossy().into_owned()))
                }),
                "/bin/sh"
            );
        });
    }

    #[test]
    fn resolve_shell_uses_bash_from_path_before_bin_sh_fallback() {
        let (tempdir, shell_path) = make_executable_shell();
        let bash_path = tempdir.path().join("bash");
        std::fs::rename(shell_path, &bash_path).unwrap();

        with_path_var(Some(tempdir.path().as_os_str()), || {
            assert_eq!(
                resolve_shell_with(None, None, || Ok(None)),
                bash_path.to_string_lossy()
            );
        });
    }
}

#[cfg(test)]
mod windows_shell_tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    use super::{resolve_windows_shell_with, shell_argv_for_platform};

    static PATH_LOCK: Mutex<()> = Mutex::new(());

    fn write_fake_executable(path: &std::path::Path) {
        std::fs::write(path, b"stub").unwrap();
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }
    }

    #[test]
    fn windows_shell_prefers_pwsh_from_path_before_legacy_powershell_and_cmd() {
        let _lock = PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tempdir = tempfile::tempdir().unwrap();
        write_fake_executable(&tempdir.path().join("pwsh.exe"));
        write_fake_executable(&tempdir.path().join("powershell.exe"));
        let path = std::env::join_paths([tempdir.path()]).unwrap();

        assert_eq!(
            resolve_windows_shell_with(
                None,
                Some(path.as_os_str()),
                Some(r"C:\Windows\System32\cmd.exe"),
            ),
            tempdir
                .path()
                .join("pwsh.exe")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn windows_shell_falls_back_to_legacy_powershell_then_comspec_then_cmd() {
        let _lock = PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tempdir = tempfile::tempdir().unwrap();
        write_fake_executable(&tempdir.path().join("powershell.exe"));
        let path = std::env::join_paths([tempdir.path()]).unwrap();

        assert_eq!(
            resolve_windows_shell_with(None, Some(path.as_os_str()), Some(r"C:\custom\cmd.exe")),
            tempdir
                .path()
                .join("powershell.exe")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(
            resolve_windows_shell_with(None, None, Some(r"C:\custom\cmd.exe")),
            r"C:\custom\cmd.exe"
        );
        assert_eq!(resolve_windows_shell_with(None, None, None), "cmd.exe");
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
