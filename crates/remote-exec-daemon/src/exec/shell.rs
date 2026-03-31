pub fn choose_shell(
    shell_override: Option<&str>,
    env_shell: Option<&str>,
    passwd_shell: Option<&str>,
) -> String {
    shell_override
        .filter(|value| !value.is_empty())
        .or_else(|| env_shell.filter(|value| !value.is_empty()))
        .or_else(|| passwd_shell.filter(|value| !value.is_empty()))
        .unwrap_or("/bin/bash")
        .to_string()
}

pub fn resolve_shell(shell_override: Option<&str>) -> anyhow::Result<String> {
    let env_shell = std::env::var("SHELL").ok();
    let passwd_shell =
        nix::unistd::User::from_uid(nix::unistd::Uid::effective())?.and_then(|user| {
            let shell = user.shell.to_string_lossy().into_owned();
            (!shell.is_empty()).then_some(shell)
        });

    Ok(choose_shell(
        shell_override,
        env_shell.as_deref(),
        passwd_shell.as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::choose_shell;

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
}
