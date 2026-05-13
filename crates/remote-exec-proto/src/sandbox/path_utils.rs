use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::path::{PathComparison, PathPolicy};

use super::SandboxError;

pub(crate) fn canonicalize_for_sandbox(path: &Path) -> Result<PathBuf, SandboxError> {
    let normalized = lexical_normalize(path);
    let mut probe = normalized.as_path();
    let mut missing_components = Vec::<OsString>::new();

    loop {
        match std::fs::canonicalize(probe) {
            Ok(canonical) => {
                let mut rebuilt = lexical_normalize(&canonical);
                for component in missing_components.iter().rev() {
                    rebuilt.push(component);
                }
                return Ok(lexical_normalize(&rebuilt));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let component = probe.file_name().ok_or_else(|| {
                    SandboxError::denied(format!(
                        "unable to resolve an existing ancestor for `{}`",
                        normalized.display()
                    ))
                })?;
                missing_components.push(component.to_os_string());
                probe = probe.parent().ok_or_else(|| {
                    SandboxError::denied(format!(
                        "unable to resolve an existing ancestor for `{}`",
                        normalized.display()
                    ))
                })?;
            }
            Err(err) => {
                return Err(SandboxError::denied(format!(
                    "unable to canonicalize `{}`: {err}",
                    normalized.display()
                )));
            }
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

pub(crate) fn path_is_within(policy: PathPolicy, root: &Path, path: &Path) -> bool {
    let mut root_components = root.components();
    let mut path_components = path.components();

    loop {
        match (root_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(root_component), Some(path_component))
                if component_eq(policy, root_component, path_component) => {}
            _ => return false,
        }
    }
}

fn component_eq(policy: PathPolicy, left: Component<'_>, right: Component<'_>) -> bool {
    let left = left.as_os_str().to_string_lossy();
    let right = right.as_os_str().to_string_lossy();

    match policy.comparison {
        PathComparison::CaseSensitive => left == right,
        PathComparison::CaseInsensitive => left.eq_ignore_ascii_case(&right),
    }
}
