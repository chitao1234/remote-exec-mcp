use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadRequest {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadResponse {
    pub image_url: String,
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::ImageReadRequest;

    #[test]
    fn image_read_request_omits_none_fields() {
        let request = ImageReadRequest {
            path: "image.png".to_string(),
            workdir: None,
            detail: None,
        };

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "path": "image.png",
            })
        );
    }
}
