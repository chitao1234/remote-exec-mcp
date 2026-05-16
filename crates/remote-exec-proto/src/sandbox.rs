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
