use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileReadRequest {
    pub path: String,
    #[serde(default)]
    pub offset: Option<u64>,
    pub limit: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileReadResponse {
    pub output: String,
    pub lines_returned: u64,
    pub total_lines: u64,
    pub eof: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileWriteRequest {
    pub path: String,
    pub content: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileWriteResponse {
    pub created: bool,
    pub line_count: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileEditRequest {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileEditResponse {
    pub replacements: u64,
    pub line_count: u64,
}
