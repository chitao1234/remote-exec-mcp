use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MIN_PORT_FORWARD_PROTOCOL_VERSION: u32 = 4;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_stream_protocol_version: Option<TransferStreamProtocolVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_tool_protocol_version: Option<FileToolProtocolVersion>,
}

impl TargetCapabilities {
    pub fn supports_compatible_port_forward(&self) -> bool {
        self.supports_port_forward
            && self
                .port_forward_protocol_version
                .is_some_and(|version| version.get() >= MIN_PORT_FORWARD_PROTOCOL_VERSION)
    }
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
        Self(NonZeroU32::new(MIN_PORT_FORWARD_PROTOCOL_VERSION).expect("v4 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct TransferStreamProtocolVersion(NonZeroU32);

impl TransferStreamProtocolVersion {
    pub fn v2() -> Self {
        Self(NonZeroU32::new(2).expect("v2 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct FileToolProtocolVersion(NonZeroU32);

impl FileToolProtocolVersion {
    pub fn v1() -> Self {
        Self(NonZeroU32::new(1).expect("v1 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FileToolProtocolVersion, PortForwardProtocolVersion, TargetCapabilities,
        TransferStreamProtocolVersion,
    };

    fn capabilities(
        supports_port_forward: bool,
        port_forward_protocol_version: Option<PortForwardProtocolVersion>,
    ) -> TargetCapabilities {
        TargetCapabilities {
            supports_pty: true,
            supports_port_forward,
            port_forward_protocol_version,
            transfer_stream_protocol_version: Some(TransferStreamProtocolVersion::v2()),
            file_tool_protocol_version: Some(FileToolProtocolVersion::v1()),
        }
    }

    #[test]
    fn compatible_port_forward_support_requires_flag_and_protocol_version() {
        assert!(
            capabilities(true, Some(PortForwardProtocolVersion::v4()))
                .supports_compatible_port_forward()
        );
        assert!(
            !capabilities(false, Some(PortForwardProtocolVersion::v4()))
                .supports_compatible_port_forward()
        );
        assert!(!capabilities(true, None).supports_compatible_port_forward());
    }
}
