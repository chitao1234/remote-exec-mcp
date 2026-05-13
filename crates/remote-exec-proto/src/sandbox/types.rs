use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    pub(crate) exec_cwd: CompiledSandboxPathList,
    pub(crate) read: CompiledSandboxPathList,
    pub(crate) write: CompiledSandboxPathList,
}

impl CompiledFilesystemSandbox {
    pub(crate) fn list(&self, access: SandboxAccess) -> &CompiledSandboxPathList {
        match access {
            SandboxAccess::ExecCwd => &self.exec_cwd,
            SandboxAccess::Read => &self.read,
            SandboxAccess::Write => &self.write,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CompiledSandboxPathList {
    pub(crate) allow: Vec<PathBuf>,
    pub(crate) deny: Vec<PathBuf>,
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
