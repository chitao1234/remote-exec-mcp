use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchInput {
    pub target: String,
    pub input: String,
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewImageInput {
    pub target: String,
    pub path: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ViewImageResult {
    pub target: String,
    pub image_url: String,
    pub detail: Option<String>,
}
