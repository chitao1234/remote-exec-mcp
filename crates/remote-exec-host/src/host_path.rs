use std::path::{Path, PathBuf};

use remote_exec_proto::path::{is_absolute_for_policy, normalize_for_system};

#[cfg(windows)]
use remote_exec_proto::path::windows_path_policy;

pub use remote_exec_proto::path::host_policy as host_path_policy;

pub fn is_input_path_absolute(raw: &str, windows_posix_root: Option<&Path>) -> bool {
    resolve_absolute_input_path(raw, windows_posix_root).is_some()
}

pub fn resolve_absolute_input_path(
    raw: &str,
    windows_posix_root: Option<&Path>,
) -> Option<PathBuf> {
    let policy = host_path_policy();
    if is_absolute_for_policy(policy, raw) {
        return Some(PathBuf::from(normalize_for_system(policy, raw)));
    }

    synthetic_windows_posix_absolute_path(raw, windows_posix_root)
}

pub fn resolve_input_path(base: &Path, raw: &str, windows_posix_root: Option<&Path>) -> PathBuf {
    let policy = host_path_policy();
    resolve_absolute_input_path(raw, windows_posix_root)
        .unwrap_or_else(|| base.join(normalize_for_system(policy, raw)))
}

#[cfg(windows)]
pub fn shell_uses_windows_posix_root(shell: &str, windows_posix_root: Option<&Path>) -> bool {
    let Some(root) = windows_posix_root else {
        return false;
    };

    let resolved = resolve_absolute_input_path(shell, Some(root)).unwrap_or_else(|| shell.into());
    crate::path_compare::path_has_prefix(&resolved, root)
}

#[cfg(not(windows))]
pub fn shell_uses_windows_posix_root(_shell: &str, _windows_posix_root: Option<&Path>) -> bool {
    false
}

#[cfg(windows)]
fn synthetic_windows_posix_absolute_path(
    raw: &str,
    windows_posix_root: Option<&Path>,
) -> Option<PathBuf> {
    let root = windows_posix_root?;
    if !raw.starts_with('/') || raw.starts_with("//") {
        return None;
    }

    let tail = raw.trim_start_matches('/');
    if tail.is_empty() {
        return Some(root.to_path_buf());
    }

    Some(root.join(normalize_for_system(windows_path_policy(), tail)))
}

#[cfg(not(windows))]
fn synthetic_windows_posix_absolute_path(
    _raw: &str,
    _windows_posix_root: Option<&Path>,
) -> Option<PathBuf> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::{
        is_input_path_absolute, resolve_absolute_input_path, resolve_input_path,
        shell_uses_windows_posix_root,
    };

    #[test]
    fn synthetic_windows_posix_root_treats_single_slash_paths_as_absolute() {
        let root = std::path::Path::new(r"C:\msys64");
        assert!(is_input_path_absolute("/usr/bin/bash", Some(root)));
        assert!(is_input_path_absolute("/", Some(root)));
        assert!(!is_input_path_absolute("/usr/bin/bash", None));
    }

    #[test]
    fn synthetic_windows_posix_root_resolves_under_configured_root() {
        let root = std::path::Path::new(r"C:\msys64");
        assert_eq!(
            resolve_absolute_input_path("/usr/bin/bash", Some(root)).unwrap(),
            std::path::PathBuf::from(r"C:\msys64\usr\bin\bash")
        );
        assert_eq!(
            resolve_absolute_input_path("/", Some(root)).unwrap(),
            std::path::PathBuf::from(r"C:\msys64")
        );
    }

    #[test]
    fn relative_paths_still_resolve_from_the_base_directory() {
        let root = std::path::Path::new(r"C:\msys64");
        assert_eq!(
            resolve_input_path(std::path::Path::new(r"C:\work"), "src/main.rs", Some(root)),
            std::path::PathBuf::from(r"C:\work\src\main.rs")
        );
    }

    #[test]
    fn shell_uses_windows_posix_root_matches_boundaries_case_insensitively() {
        let root = std::path::Path::new(r"C:\msys64");
        assert!(shell_uses_windows_posix_root(
            r"C:\MSYS64\usr\bin\zsh.exe",
            Some(root)
        ));
        assert!(!shell_uses_windows_posix_root(
            r"C:\msys64-tools\usr\bin\zsh.exe",
            Some(root)
        ));
    }
}
