use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use remote_exec_proto::path::is_absolute_for_policy;
use remote_exec_proto::sandbox::{FilesystemSandbox, SandboxPathList};

use crate::{host_path, path_compare};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    Denied { message: String },
    NotAbsolute { path: PathBuf },
}

impl SandboxError {
    pub(crate) fn denied(message: impl Into<String>) -> Self {
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

pub fn compile_filesystem_sandbox(
    sandbox: &FilesystemSandbox,
) -> Result<CompiledFilesystemSandbox, SandboxError> {
    Ok(CompiledFilesystemSandbox {
        exec_cwd: compile_list("exec_cwd", &sandbox.exec_cwd)?,
        read: compile_list("read", &sandbox.read)?,
        write: compile_list("write", &sandbox.write)?,
    })
}

pub fn authorize_path(
    sandbox: Option<&CompiledFilesystemSandbox>,
    access: SandboxAccess,
    path: &Path,
) -> Result<(), SandboxError> {
    let policy = host_path::host_path_policy();
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
        .find(|deny_root| path_compare::path_is_within(&resolved, deny_root))
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
            .any(|allow_root| path_compare::path_is_within(&resolved, allow_root))
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
    label: &str,
    list: &SandboxPathList,
) -> Result<CompiledSandboxPathList, SandboxError> {
    Ok(CompiledSandboxPathList {
        allow: list
            .allow
            .iter()
            .map(|entry| compile_root(label, "allow", entry))
            .collect::<Result<Vec<_>, _>>()?,
        deny: list
            .deny
            .iter()
            .map(|entry| compile_root(label, "deny", entry))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn compile_root(access_label: &str, list_label: &str, raw: &str) -> Result<PathBuf, SandboxError> {
    let policy = host_path::host_path_policy();
    if !is_absolute_for_policy(policy, raw) {
        return Err(SandboxError::denied(format!(
            "sandbox {access_label}.{list_label} path `{raw}` is not absolute"
        )));
    }

    let normalized = PathBuf::from(remote_exec_proto::path::normalize_for_system(policy, raw));
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use remote_exec_proto::sandbox::{FilesystemSandbox, SandboxPathList};

    use super::{
        CompiledFilesystemSandbox, SandboxAccess, SandboxError, authorize_path,
        compile_filesystem_sandbox,
    };

    #[test]
    fn authorize_path_rejects_relative_path_with_distinct_error() {
        let sandbox = CompiledFilesystemSandbox::default();
        let err = authorize_path(
            Some(&sandbox),
            SandboxAccess::Read,
            Path::new("relative/path"),
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
        let compiled = compile_filesystem_sandbox(&sandbox).unwrap();

        assert!(
            authorize_path(
                Some(&compiled),
                SandboxAccess::Read,
                &tempdir.path().join("allowed.txt"),
            )
            .is_ok()
        );
        assert!(
            authorize_path(
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
        let compiled = compile_filesystem_sandbox(&sandbox).unwrap();

        assert!(
            authorize_path(
                Some(&compiled),
                SandboxAccess::Write,
                &allowed.join("ok.txt"),
            )
            .is_ok()
        );
        assert!(
            authorize_path(
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
        let compiled = compile_filesystem_sandbox(&sandbox).unwrap();

        assert!(
            authorize_path(
                Some(&compiled),
                SandboxAccess::Read,
                &tempdir.path().join("artifact.txt"),
            )
            .is_ok()
        );
    }
}
