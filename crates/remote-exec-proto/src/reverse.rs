use serde::{Deserialize, Serialize};

pub const REVERSE_PROTOCOL_MAGIC: &[u8; 8] = b"REXREV1\n";
pub const REVERSE_PROTOCOL_VERSION: u16 = 1;
pub const MAX_REVERSE_REGISTRATION_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReverseRegistration {
    pub protocol_version: u16,
    pub target: String,
    pub daemon_instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReverseRegistrationAck {
    pub accepted: bool,
    pub message: String,
}

impl ReverseRegistration {
    pub fn new(target: String, daemon_instance_id: String, bearer_token: Option<String>) -> Self {
        Self {
            protocol_version: REVERSE_PROTOCOL_VERSION,
            target,
            daemon_instance_id,
            bearer_token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_round_trips() {
        let registration = ReverseRegistration::new(
            "builder-a".to_string(),
            "instance-a".to_string(),
            Some("secret".to_string()),
        );
        let encoded = serde_json::to_vec(&registration).unwrap();
        assert_eq!(
            serde_json::from_slice::<ReverseRegistration>(&encoded).unwrap(),
            registration
        );
    }
}
