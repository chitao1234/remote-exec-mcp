use std::path::{Path, PathBuf};

use crate::path::{PathPolicy, is_absolute_for_policy, normalize_for_system};

use super::path_utils::{canonicalize_for_sandbox, path_is_within};
use super::types::{CompiledFilesystemSandbox, CompiledSandboxPathList};
use super::{FilesystemSandbox, SandboxAccess, SandboxError, SandboxPathList};

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
