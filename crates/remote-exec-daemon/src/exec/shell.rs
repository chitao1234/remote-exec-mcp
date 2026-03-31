use std::path::Path;

pub fn choose_shell(
    shell_override: Option<&str>,
    env_shell: Option<&str>,
    passwd_shell: Option<&str>,
) -> String {
    shell_override
        .filter(|value| !value.is_empty())
        .or_else(|| env_shell.filter(|value| !value.is_empty()))
        .or_else(|| usable_passwd_shell(passwd_shell))
        .unwrap_or("/bin/bash")
        .to_string()
}

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

fn resolve_shell_with<F>(
    shell_override: Option<&str>,
    env_shell: Option<&str>,
    passwd_shell_lookup: F,
) -> String
where
    F: FnOnce() -> anyhow::Result<Option<String>>,
{
    let passwd_shell = passwd_shell_lookup().ok().flatten();
    choose_shell(shell_override, env_shell, passwd_shell.as_deref())
}

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

#[cfg(not(unix))]
fn has_execute_bits(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{choose_shell, resolve_shell_with};

    #[test]
    fn choose_shell_prefers_explicit_override() {
        assert_eq!(
            choose_shell(Some("/bin/zsh"), Some("/bin/sh"), Some("/bin/bash")),
            "/bin/zsh"
        );
    }

    #[test]
    fn choose_shell_uses_env_before_passwd_and_fallback() {
        assert_eq!(
            choose_shell(None, Some("/bin/sh"), Some("/bin/bash")),
            "/bin/sh"
        );
        assert_eq!(choose_shell(None, None, Some("/bin/bash")), "/bin/bash");
        assert_eq!(choose_shell(None, None, None), "/bin/bash");
    }

    #[test]
    fn choose_shell_ignores_unusable_passwd_shells() {
        assert_eq!(
            choose_shell(None, Some("/bin/sh"), Some("/usr/sbin/nologin")),
            "/bin/sh"
        );
        assert_eq!(choose_shell(None, None, Some("/bin/false")), "/bin/bash");
    }

    #[test]
    fn resolve_shell_ignores_passwd_lookup_failure() {
        assert_eq!(
            resolve_shell_with(None, Some("/bin/sh"), || {
                Err(anyhow::anyhow!("passwd lookup failed"))
            }),
            "/bin/sh"
        );
        assert_eq!(
            resolve_shell_with(None, None, || Err(anyhow::anyhow!("passwd lookup failed"))),
            "/bin/bash"
        );
    }

    #[test]
    fn resolve_shell_ignores_missing_passwd_shell() {
        assert_eq!(
            resolve_shell_with(None, None, || {
                Ok(Some("/opt/missing-shell".to_string()))
            }),
            "/bin/bash"
        );
    }

    #[test]
    fn resolve_shell_ignores_non_executable_passwd_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        let shell_path = tempdir.path().join("shell");
        std::fs::write(&shell_path, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&shell_path).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&shell_path, permissions).unwrap();

        assert_eq!(
            resolve_shell_with(None, None, || {
                Ok(Some(shell_path.to_string_lossy().into_owned()))
            }),
            "/bin/bash"
        );
    }
}
