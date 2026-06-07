use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use remote_exec_proto::path::windows_path_policy;

pub use remote_exec_proto::path::host_policy as host_path_policy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHostPath {
    raw: String,
    path: PathBuf,
}

impl ResolvedHostPath {
    pub fn new(raw: impl Into<String>, path: PathBuf) -> Self {
        Self {
            raw: raw.into(),
            path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.path
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn display_or_raw(&self) -> String {
        if self.raw.is_empty() {
            self.path.display().to_string()
        } else {
            self.raw.clone()
        }
    }
}

pub(crate) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub fn is_input_path_absolute(raw: &str, windows_posix_root: Option<&Path>) -> bool {
    resolve_absolute_input_path(raw, windows_posix_root).is_some()
}

pub fn resolve_absolute_input_path(
    raw: &str,
    windows_posix_root: Option<&Path>,
) -> Option<PathBuf> {
    let policy = host_path_policy();
    if policy.is_absolute(raw) {
        return Some(PathBuf::from(policy.normalize_for_system(raw)));
    }

    synthetic_windows_posix_absolute_path(raw, windows_posix_root)
}

pub fn resolve_input_path(base: &Path, raw: &str, windows_posix_root: Option<&Path>) -> PathBuf {
    let policy = host_path_policy();
    resolve_absolute_input_path(raw, windows_posix_root)
        .unwrap_or_else(|| base.join(policy.normalize_for_system(raw)))
}

pub fn resolve_input_path_for_operation(
    base: &Path,
    raw: &str,
    windows_posix_root: Option<&Path>,
) -> ResolvedHostPath {
    ResolvedHostPath::new(
        raw,
        lexical_normalize(&resolve_input_path(base, raw, windows_posix_root)),
    )
}

pub fn resolve_path_text_for_operation(
    raw: &str,
    windows_posix_root: Option<&Path>,
) -> ResolvedHostPath {
    let policy = host_path_policy();
    let path = resolve_absolute_input_path(raw, windows_posix_root)
        .unwrap_or_else(|| PathBuf::from(policy.normalize_for_system(raw)));
    ResolvedHostPath::new(raw, path)
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

    Some(root.join(windows_path_policy().normalize_for_system(tail)))
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
