use std::fmt;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
const REQUEST_ID_PREFIX: &str = "req_";
const MAX_REQUEST_ID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    pub fn new() -> Self {
        Self(format!(
            "{REQUEST_ID_PREFIX}{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    pub fn from_header_value(value: &str) -> Option<Self> {
        is_log_safe_request_id(value).then(|| Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn is_log_safe_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_LEN
        && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{REQUEST_ID_HEADER, REQUEST_ID_PREFIX, RequestId};

    #[test]
    fn request_id_header_uses_standard_name() {
        assert_eq!(REQUEST_ID_HEADER, "x-request-id");
    }

    #[test]
    fn generated_request_ids_are_prefixed_and_log_safe() {
        let id = RequestId::new();

        assert!(id.as_str().starts_with(REQUEST_ID_PREFIX));
        assert_eq!(id.as_str().len(), REQUEST_ID_PREFIX.len() + 32);
        assert_eq!(
            RequestId::from_header_value(id.as_str()).unwrap().as_str(),
            id.as_str()
        );
    }

    #[test]
    fn header_value_parser_accepts_common_visible_ascii_ids() {
        let id = RequestId::from_header_value("client-123_abc.456:789").unwrap();

        assert_eq!(id.as_str(), "client-123_abc.456:789");
    }

    #[test]
    fn header_value_parser_rejects_empty_control_whitespace_and_overlong_values() {
        for value in ["", "has space", "line\nbreak", "\tindent"] {
            assert!(RequestId::from_header_value(value).is_none());
        }

        let overlong = "a".repeat(129);
        assert!(RequestId::from_header_value(&overlong).is_none());
    }
}
