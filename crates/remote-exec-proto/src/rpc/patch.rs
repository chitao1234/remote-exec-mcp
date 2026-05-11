use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyRequest {
    pub patch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyResponse {
    pub output: String,
}

#[cfg(test)]
mod tests {
    use super::PatchApplyRequest;

    #[test]
    fn patch_apply_request_omits_none_fields() {
        let request = PatchApplyRequest {
            patch: "*** Begin Patch\n*** End Patch\n".to_string(),
            workdir: None,
        };

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "patch": "*** Begin Patch\n*** End Patch\n",
            })
        );
    }
}
