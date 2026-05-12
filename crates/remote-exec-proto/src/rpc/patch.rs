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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updated_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{PatchApplyRequest, PatchApplyResponse};

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

    #[test]
    fn patch_apply_response_omits_empty_audit_fields() {
        let response = PatchApplyResponse {
            output: "Success.\n".to_string(),
            daemon_instance_id: None,
            updated_paths: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            serde_json::json!({
                "output": "Success.\n",
            })
        );
    }

    #[test]
    fn patch_apply_response_defaults_missing_audit_fields() {
        let response: PatchApplyResponse = serde_json::from_value(serde_json::json!({
            "output": "Success.\n",
        }))
        .unwrap();

        assert_eq!(response.output, "Success.\n");
        assert_eq!(response.daemon_instance_id, None);
        assert!(response.updated_paths.is_empty());
    }
}
