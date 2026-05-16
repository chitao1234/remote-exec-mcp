use std::borrow::Borrow;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ForwardId(String);

impl ForwardId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ForwardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ForwardId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ForwardId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for ForwardId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ForwardId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl PartialEq<str> for ForwardId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortForwardProtoError {
    #[error("endpoint must not be empty")]
    EmptyEndpoint,
    #[error("invalid endpoint `{endpoint}`; missing `]`")]
    MissingIpv6Bracket { endpoint: String },
    #[error("invalid endpoint `{endpoint}`; expected [host]:port")]
    InvalidIpv6Endpoint { endpoint: String },
    #[error("invalid endpoint `{endpoint}`; expected <port> or <host>:<port>")]
    InvalidEndpoint { endpoint: String },
    #[error("endpoint host must not be empty")]
    EmptyHost,
    #[error("invalid port `{value}`: {message}")]
    InvalidPort { value: String, message: String },
    #[error("connect_endpoint `{endpoint}` must use a nonzero port")]
    ZeroConnectPort { endpoint: String },
}

pub const DEFAULT_TUNNEL_QUEUE_BYTES: u64 = 8 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, PortForwardProtoError>;

pub fn normalize_endpoint(endpoint: &str) -> Result<String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(PortForwardProtoError::EmptyEndpoint);
    }
    if endpoint.chars().all(|value| value.is_ascii_digit()) {
        let port = parse_port(endpoint)?;
        return Ok(format!("127.0.0.1:{port}"));
    }
    validate_host_port(endpoint)?;
    Ok(endpoint.to_string())
}

pub fn ensure_nonzero_connect_endpoint(endpoint: &str) -> Result<String> {
    let endpoint = normalize_endpoint(endpoint)?;
    let port = endpoint_port(&endpoint)?;
    if port == 0 {
        return Err(PortForwardProtoError::ZeroConnectPort { endpoint });
    }
    Ok(endpoint)
}

pub fn endpoint_port(endpoint: &str) -> Result<u16> {
    let (_, port) = split_host_port(endpoint)?;
    parse_port(port)
}

pub fn udp_connector_endpoint(connect_endpoint: &str) -> Result<&'static str> {
    let normalized = normalize_endpoint(connect_endpoint)?;
    if normalized.starts_with('[') {
        return Ok("[::]:0");
    }
    Ok("0.0.0.0:0")
}

fn validate_host_port(endpoint: &str) -> Result<()> {
    let (host, port) = split_host_port(endpoint)?;
    if host.is_empty() {
        return Err(PortForwardProtoError::EmptyHost);
    }
    parse_port(port)?;
    Ok(())
}

fn split_host_port(endpoint: &str) -> Result<(&str, &str)> {
    if let Some(rest) = endpoint.strip_prefix('[') {
        let (host, suffix) =
            rest.split_once(']')
                .ok_or_else(|| PortForwardProtoError::MissingIpv6Bracket {
                    endpoint: endpoint.to_string(),
                })?;
        let port =
            suffix
                .strip_prefix(':')
                .ok_or_else(|| PortForwardProtoError::InvalidIpv6Endpoint {
                    endpoint: endpoint.to_string(),
                })?;
        return Ok((host, port));
    }

    endpoint
        .rsplit_once(':')
        .ok_or_else(|| PortForwardProtoError::InvalidEndpoint {
            endpoint: endpoint.to_string(),
        })
}

fn parse_port(value: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .map_err(|err| PortForwardProtoError::InvalidPort {
            value: value.to_string(),
            message: err.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::{
        PortForwardProtoError, endpoint_port, ensure_nonzero_connect_endpoint, normalize_endpoint,
    };

    #[test]
    fn bare_port_normalizes_to_ipv4_loopback() {
        assert_eq!(normalize_endpoint("8080").unwrap(), "127.0.0.1:8080");
    }

    #[test]
    fn ipv6_endpoint_is_accepted() {
        assert_eq!(normalize_endpoint("[::1]:8080").unwrap(), "[::1]:8080");
    }

    #[test]
    fn connect_endpoint_rejects_zero_port() {
        let err = ensure_nonzero_connect_endpoint("127.0.0.1:0").unwrap_err();
        assert!(
            err.to_string().contains("nonzero port"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hostname_endpoint_is_accepted() {
        assert_eq!(
            normalize_endpoint("localhost:8080").unwrap(),
            "localhost:8080"
        );
    }

    #[test]
    fn invalid_endpoint_returns_typed_error() {
        let err = normalize_endpoint("localhost").unwrap_err();
        assert!(matches!(err, PortForwardProtoError::InvalidEndpoint { .. }));
    }

    #[test]
    fn invalid_port_returns_typed_error() {
        let err = endpoint_port("127.0.0.1:not-a-port").unwrap_err();
        assert!(matches!(err, PortForwardProtoError::InvalidPort { .. }));
    }
}
