mod common;
#[cfg(unix)]
mod unix;
#[cfg(any(test, windows))]
mod windows;

use std::path::Path;

use crate::config::ProcessEnvironment;

pub use common::{apply_session_environment_overrides, shell_argv};

pub fn platform_supports_login_shells() -> bool {
    true
}

#[cfg(unix)]
pub fn resolve_default_shell(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    unix::resolve_default_shell(configured_default_shell, environment, windows_posix_root)
}

#[cfg(windows)]
pub fn resolve_default_shell(
    configured_default_shell: Option<&str>,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    windows::resolve_default_shell(configured_default_shell, environment, windows_posix_root)
}

#[cfg(unix)]
pub fn selected_shell(
    shell_override: Option<&str>,
    default_shell: &str,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    unix::selected_shell(
        shell_override,
        default_shell,
        environment,
        windows_posix_root,
    )
}

#[cfg(windows)]
pub fn selected_shell(
    shell_override: Option<&str>,
    default_shell: &str,
    environment: &ProcessEnvironment,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<String> {
    windows::selected_shell(
        shell_override,
        default_shell,
        environment,
        windows_posix_root,
    )
}
