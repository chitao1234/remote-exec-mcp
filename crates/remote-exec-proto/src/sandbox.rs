use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::path::{PathComparison, PathPolicy, is_absolute_for_policy, normalize_for_system};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct FilesystemSandbox {
    #[serde(default)]
    pub exec_cwd: SandboxPathList,
    #[serde(default)]
    pub read: SandboxPathList,
    #[serde(default)]
    pub write: SandboxPathList,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SandboxPathList {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxAccess {
    ExecCwd,
    Read,
    Write,
}

impl SandboxAccess {
    pub fn label(self) -> &'static str {
        match self {
            Self::ExecCwd => "exec_cwd",
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompiledFilesystemSandbox {
    exec_cwd: CompiledSandboxPathList,
    read: CompiledSandboxPathList,
    write: CompiledSandboxPathList,
}

impl CompiledFilesystemSandbox {
    fn list(&self, access: SandboxAccess) -> &CompiledSandboxPathList {
        match access {
            SandboxAccess::ExecCwd => &self.exec_cwd,
            SandboxAccess::Read => &self.read,
            SandboxAccess::Write => &self.write,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CompiledSandboxPathList {
    allow: Vec<PathBuf>,
    deny: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    Denied { message: String },
    NotAbsolute { path: PathBuf },
}

impl SandboxError {
    fn denied(message: impl Into<String>) -> Self {
        Self::Denied {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied { message } => f.write_str(message),
            Self::NotAbsolute { path } => write!(f, "path `{}` is not absolute", path.display()),
        }
    }
}

impl std::error::Error for SandboxError {}

pub fn compile_filesystem_sandbox(
    policy: PathPolicy,
    sandbox: &FilesystemSandbox,
) -> Result<CompiledFilesystemSandbox, SandboxError> {
    Ok(CompiledFilesystemSandbox {
        exec_cwd: compile_list(policy, "exec_cwd", &sandbox.exec_cwd)?,
        read: compile_list(policy, "read", &sandbox.read)?,
        write: compile_list(policy, "write", &sandbox.write)?,
    })
}

pub fn authorize_path(
    policy: PathPolicy,
    sandbox: Option<&CompiledFilesystemSandbox>,
    access: SandboxAccess,
    path: &Path,
) -> Result<(), SandboxError> {
    if !is_absolute_for_policy(policy, &path.to_string_lossy()) {
        return Err(SandboxError::NotAbsolute {
            path: path.to_path_buf(),
        });
    }

    let Some(sandbox) = sandbox else {
        return Ok(());
    };

    let resolved = canonicalize_for_sandbox(path)?;
    let rules = sandbox.list(access);
    if let Some(deny_root) = rules
        .deny
        .iter()
        .find(|deny_root| path_is_within(policy, deny_root, &resolved))
    {
        return Err(SandboxError::denied(format!(
            "{} access to `{}` is denied by sandbox rule `{}`",
            access.label(),
            resolved.display(),
            deny_root.display()
        )));
    }

    if rules.allow.is_empty()
        || rules
            .allow
            .iter()
            .any(|allow_root| path_is_within(policy, allow_root, &resolved))
    {
        return Ok(());
    }

    Err(SandboxError::denied(format!(
        "{} access to `{}` is outside the configured sandbox",
        access.label(),
        resolved.display()
    )))
}

fn compile_list(
    policy: PathPolicy,
    label: &str,
    list: &SandboxPathList,
) -> Result<CompiledSandboxPathList, SandboxError> {
    Ok(CompiledSandboxPathList {
        allow: list
            .allow
            .iter()
            .map(|entry| compile_root(policy, label, "allow", entry))
            .collect::<Result<Vec<_>, _>>()?,
        deny: list
            .deny
            .iter()
            .map(|entry| compile_root(policy, label, "deny", entry))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn compile_root(
    policy: PathPolicy,
    access_label: &str,
    list_label: &str,
    raw: &str,
) -> Result<PathBuf, SandboxError> {
    if !is_absolute_for_policy(policy, raw) {
        return Err(SandboxError::denied(format!(
            "sandbox {access_label}.{list_label} path `{raw}` is not absolute"
        )));
    }

    let normalized = PathBuf::from(normalize_for_system(policy, raw));
    canonicalize_for_sandbox(&normalized).map_err(|err| {
        SandboxError::denied(format!(
            "sandbox {access_label}.{list_label} path `{}` is invalid: {err}",
            normalized.display()
        ))
    })
}

fn canonicalize_for_sandbox(path: &Path) -> Result<PathBuf, SandboxError> {
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

fn lexical_normalize(path: &Path) -> PathBuf {
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

fn path_is_within(policy: PathPolicy, root: &Path, path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{
        CompiledFilesystemSandbox, FilesystemSandbox, SandboxAccess, SandboxError, SandboxPathList,
        authorize_path, compile_filesystem_sandbox,
    };
    #[cfg(not(windows))]
    use crate::path::linux_path_policy;
    #[cfg(windows)]
    use crate::path::windows_path_policy;

    fn host_path_policy() -> crate::path::PathPolicy {
        #[cfg(windows)]
        {
            windows_path_policy()
        }
        #[cfg(not(windows))]
        {
            linux_path_policy()
        }
    }

    #[test]
    fn authorize_path_rejects_relative_path_with_distinct_error() {
        let sandbox = CompiledFilesystemSandbox::default();
        let err = authorize_path(
            crate::path::linux_path_policy(),
            Some(&sandbox),
            SandboxAccess::Read,
            std::path::Path::new("relative/path"),
        )
        .expect_err("relative path should be rejected");
        assert!(matches!(err, SandboxError::NotAbsolute { .. }));
    }

    #[test]
    fn empty_allow_list_defaults_to_allow_all() {
        let tempdir = tempfile::tempdir().unwrap();
        let nested = tempdir.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        let sandbox = FilesystemSandbox {
            read: SandboxPathList {
                allow: Vec::new(),
                deny: vec![nested.display().to_string()],
            },
            ..Default::default()
        };
        let policy = host_path_policy();
        let compiled = compile_filesystem_sandbox(policy, &sandbox).unwrap();

        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Read,
                &tempdir.path().join("allowed.txt"),
            )
            .is_ok()
        );
        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Read,
                &nested.join("blocked.txt"),
            )
            .is_err()
        );
    }

    #[test]
    fn non_empty_allow_list_requires_membership() {
        let tempdir = tempfile::tempdir().unwrap();
        let allowed = tempdir.path().join("allowed");
        let denied = tempdir.path().join("denied");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();

        let sandbox = FilesystemSandbox {
            write: SandboxPathList {
                allow: vec![allowed.display().to_string()],
                deny: Vec::new(),
            },
            ..Default::default()
        };
        let policy = host_path_policy();
        let compiled = compile_filesystem_sandbox(policy, &sandbox).unwrap();

        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Write,
                &allowed.join("ok.txt"),
            )
            .is_ok()
        );
        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Write,
                &denied.join("nope.txt"),
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively() {
        let tempdir = tempfile::tempdir().unwrap();
        let sandbox = FilesystemSandbox {
            read: SandboxPathList {
                allow: vec![tempdir.path().display().to_string().to_uppercase()],
                deny: Vec::new(),
            },
            ..Default::default()
        };
        let compiled = compile_filesystem_sandbox(windows_path_policy(), &sandbox).unwrap();

        assert!(
            authorize_path(
                windows_path_policy(),
                Some(&compiled),
                SandboxAccess::Read,
                &tempdir.path().join("artifact.txt"),
            )
            .is_ok()
        );
    }
}
