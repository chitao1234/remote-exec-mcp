use std::ffi::OsStr;
use std::path::Path;

use anyhow::Context;

use crate::config::ProcessEnvironment;

use super::common::{is_path_like, probe_shell_for_platform};

pub(super) fn resolve_default_shell(
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

pub(super) fn selected_shell(
    shell_override: Option<&str>,
    default_shell: &str,
    _environment: &ProcessEnvironment,
) -> anyhow::Result<String> {
    Ok(shell_override
        .filter(|value| !value.is_empty())
        .unwrap_or(default_shell)
        .to_string())
}

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

fn is_disallowed_unix_shell(shell: &str) -> bool {
    matches!(
        Path::new(shell).file_name().and_then(|name| name.to_str()),
        Some("false" | "nologin")
    )
}

fn find_unix_command_on_path(path_env: Option<&OsStr>, command: &str) -> Option<String> {
    std::env::split_paths(path_env?)
        .map(|dir| dir.join(command))
        .find(|path| shell_path_is_executable(path))
        .map(|path| path.to_string_lossy().into_owned())
}

fn shell_path_is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    metadata.is_file() && has_execute_bits(&metadata)
}

fn has_execute_bits(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
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
