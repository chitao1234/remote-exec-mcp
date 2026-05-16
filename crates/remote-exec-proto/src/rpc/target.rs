use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthCheckResponse {
    pub status: HealthStatus,
    pub daemon_version: String,
    pub daemon_instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DaemonIdentity {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TargetCapabilities {
    pub supports_pty: bool,
    #[serde(default)]
    pub supports_port_forward: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_forward_protocol_version: Option<PortForwardProtocolVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInfoResponse {
    pub target: String,
    pub daemon_instance_id: String,
    #[serde(flatten)]
    pub identity: DaemonIdentity,
    #[serde(flatten)]
    pub capabilities: TargetCapabilities,
    pub supports_image_read: bool,
    #[serde(default)]
    pub supports_transfer_compression: bool,
}

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct PortForwardProtocolVersion(NonZeroU32);

impl PortForwardProtocolVersion {
    pub fn v4() -> Self {
        Self(NonZeroU32::new(4).expect("v4 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}
