use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Context;

use crate::config::ProcessEnvironment;
use crate::exec::session::SpawnCommand;
use crate::host_path;

#[allow(unexpected_cfgs)]
pub fn apply_session_environment_overrides(
    environment: &mut ProcessEnvironment,
    shell: &str,
    windows_posix_root: Option<&Path>,
) {
    if should_set_chere_invoking_for_platform(
        cfg!(windows),
        cfg!(target_os = "cygwin"),
        shell,
        windows_posix_root,
    ) {
        environment.set_var("CHERE_INVOKING", Some("1".into()));
    }
}

pub fn shell_command(shell: &str, login: bool, cmd: &str) -> SpawnCommand {
    shell_command_for_platform(cfg!(windows), shell, login, cmd)
}

fn shell_command_with_login_flag(shell: &str, login: bool, cmd: &str) -> SpawnCommand {
    let mut args = Vec::new();
    if login {
        args.push("-l".to_string());
    }
    args.push("-c".to_string());
    args.push(cmd.to_string());
    SpawnCommand {
        program: shell.to_string(),
        argv0: None,
        args,
    }
}

fn unix_shell_command(shell: &str, login: bool, cmd: &str) -> SpawnCommand {
    SpawnCommand {
        program: shell.to_string(),
        argv0: login.then(|| format!("-{}", shell_basename(shell))),
        args: vec!["-c".to_string(), cmd.to_string()],
    }
}

pub(super) fn shell_command_for_platform(
    is_windows: bool,
    shell: &str,
    login: bool,
    cmd: &str,
) -> SpawnCommand {
    let lower = shell_basename_lower(shell);

    if is_windows_powershell_family(&lower) {
        let mut args = Vec::new();
        if !login {
            args.push("-NoProfile".to_string());
        }
        args.push("-Command".to_string());
        args.push(cmd.to_string());
        return SpawnCommand {
            program: shell.to_string(),
            argv0: None,
            args,
        };
    }

    if is_windows && is_windows_bash_family(&lower) {
        return shell_command_with_login_flag(shell, login, cmd);
    }

    if is_windows {
        let mut args = Vec::new();
        if is_windows_cmd_family(&lower) {
            if !login {
                args.push("/D".to_string());
            }
            args.push("/C".to_string());
            args.push(cmd.to_string());
            return SpawnCommand {
                program: shell.to_string(),
                argv0: None,
                args,
            };
        }
        args.push("/C".to_string());
        args.push(cmd.to_string());
        return SpawnCommand {
            program: shell.to_string(),
            argv0: None,
            args,
        };
    }

    unix_shell_command(shell, login, cmd)
}

pub(super) fn probe_shell_for_platform(
    is_windows: bool,
    shell: &str,
    environment: &ProcessEnvironment,
) -> anyhow::Result<()> {
    let command_spec = shell_command_for_platform(is_windows, shell, false, "exit 0");
    let mut command = Command::new(&command_spec.program);
    command.args(&command_spec.args);
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

pub(super) fn is_path_like(command: &str) -> bool {
    let path = Path::new(command);
    path.is_absolute() || path.components().count() > 1
}

pub(super) fn shell_basename_lower(shell: &str) -> String {
    shell_basename(shell).to_ascii_lowercase()
}

fn shell_basename(shell: &str) -> &str {
    shell.rsplit(['\\', '/']).next().unwrap_or(shell)
}

pub(super) fn is_windows_powershell_family(lower: &str) -> bool {
    matches!(lower, "powershell.exe" | "powershell" | "pwsh.exe" | "pwsh")
}

pub(super) fn is_windows_cmd_family(lower: &str) -> bool {
    matches!(lower, "cmd.exe" | "cmd")
}

pub(super) fn is_windows_bash_family(lower: &str) -> bool {
    matches!(
        lower,
        "bash.exe" | "bash" | "sh.exe" | "sh" | "git-bash.exe" | "git-bash"
    )
}

#[cfg(any(test, windows))]
pub(super) fn is_windows_git_bash_alias(lower: &str) -> bool {
    is_windows_bash_family(lower)
}

#[cfg(any(test, unix, windows))]
pub(super) fn should_set_chere_invoking_for_platform(
    is_windows: bool,
    is_cygwin: bool,
    shell: &str,
    windows_posix_root: Option<&Path>,
) -> bool {
    if is_cygwin {
        return true;
    }

    let lower = shell_basename_lower(shell);

    if is_windows {
        return is_windows_bash_family(&lower)
            || host_path::shell_uses_windows_posix_root(shell, windows_posix_root);
    }

    false
}

#[cfg(test)]
mod tests {
    use crate::exec::session::SpawnCommand;

    use super::{shell_command_for_platform, should_set_chere_invoking_for_platform};

    #[cfg(unix)]
    #[test]
    fn unix_shell_command_uses_dash_c_for_non_login_shells() {
        assert_eq!(
            shell_command_for_platform(false, "/bin/sh", false, "printf ok"),
            SpawnCommand {
                program: "/bin/sh".to_string(),
                argv0: None,
                args: vec!["-c".to_string(), "printf ok".to_string()],
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_shell_command_uses_login_argv0_for_login_shells() {
        assert_eq!(
            shell_command_for_platform(false, "/bin/sh", true, "printf ok"),
            SpawnCommand {
                program: "/bin/sh".to_string(),
                argv0: Some("-sh".to_string()),
                args: vec!["-c".to_string(), "printf ok".to_string()],
            }
        );
    }

    #[test]
    fn windows_shell_command_suppresses_profiles_and_autorun_only_for_non_login_requests() {
        assert_eq!(
            shell_command_for_platform(true, "pwsh.exe", false, "Write-Output ok"),
            SpawnCommand {
                program: "pwsh.exe".to_string(),
                argv0: None,
                args: vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "Write-Output ok".to_string(),
                ],
            }
        );
        assert_eq!(
            shell_command_for_platform(true, "pwsh.exe", true, "Write-Output ok"),
            SpawnCommand {
                program: "pwsh.exe".to_string(),
                argv0: None,
                args: vec!["-Command".to_string(), "Write-Output ok".to_string()],
            }
        );
        assert_eq!(
            shell_command_for_platform(true, "cmd.exe", false, "echo ok"),
            SpawnCommand {
                program: "cmd.exe".to_string(),
                argv0: None,
                args: vec!["/D".to_string(), "/C".to_string(), "echo ok".to_string()],
            }
        );
        assert_eq!(
            shell_command_for_platform(true, "cmd.exe", true, "echo ok"),
            SpawnCommand {
                program: "cmd.exe".to_string(),
                argv0: None,
                args: vec!["/C".to_string(), "echo ok".to_string()],
            }
        );
        assert_eq!(
            shell_command_for_platform(true, "bash.exe", false, "printf ok"),
            SpawnCommand {
                program: "bash.exe".to_string(),
                argv0: None,
                args: vec!["-c".to_string(), "printf ok".to_string()],
            }
        );
        assert_eq!(
            shell_command_for_platform(true, "bash.exe", true, "printf ok"),
            SpawnCommand {
                program: "bash.exe".to_string(),
                argv0: None,
                args: vec!["-l".to_string(), "-c".to_string(), "printf ok".to_string()],
            }
        );
    }

    #[test]
    fn chere_invoking_applies_to_all_cygwin_shells() {
        assert!(should_set_chere_invoking_for_platform(
            false,
            true,
            "/bin/bash",
            None
        ));
        assert!(should_set_chere_invoking_for_platform(
            false, true, "sh", None
        ));
        assert!(should_set_chere_invoking_for_platform(
            false, true, "/bin/zsh", None
        ));
    }

    #[cfg(windows)]
    #[test]
    fn chere_invoking_applies_to_windows_posix_root_shells_even_when_not_bash_family() {
        let root = std::path::Path::new(r"C:\msys64");
        assert!(should_set_chere_invoking_for_platform(
            true,
            false,
            "/usr/bin/zsh",
            Some(root)
        ));
    }
}
