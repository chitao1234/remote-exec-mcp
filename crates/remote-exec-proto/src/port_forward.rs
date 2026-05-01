use anyhow::{Context, anyhow};

pub fn normalize_endpoint(endpoint: &str) -> anyhow::Result<String> {
    let endpoint = endpoint.trim();
    anyhow::ensure!(!endpoint.is_empty(), "endpoint must not be empty");
    if endpoint.chars().all(|value| value.is_ascii_digit()) {
        let port = parse_port(endpoint)?;
        return Ok(format!("127.0.0.1:{port}"));
    }
    validate_host_port(endpoint)?;
    Ok(endpoint.to_string())
}

pub fn ensure_nonzero_connect_endpoint(endpoint: &str) -> anyhow::Result<String> {
    let endpoint = normalize_endpoint(endpoint)?;
    let port = endpoint_port(&endpoint)?;
    anyhow::ensure!(
        port != 0,
        "connect_endpoint `{endpoint}` must use a nonzero port"
    );
    Ok(endpoint)
}

pub fn endpoint_port(endpoint: &str) -> anyhow::Result<u16> {
    let (_, port) = split_host_port(endpoint)?;
    parse_port(port)
}

pub fn udp_connector_endpoint(connect_endpoint: &str) -> anyhow::Result<&'static str> {
    let normalized = normalize_endpoint(connect_endpoint)?;
    if normalized.starts_with('[') {
        return Ok("[::]:0");
    }
    Ok("0.0.0.0:0")
}

fn validate_host_port(endpoint: &str) -> anyhow::Result<()> {
    let (host, port) = split_host_port(endpoint)?;
    anyhow::ensure!(!host.is_empty(), "endpoint host must not be empty");
    parse_port(port)?;
    Ok(())
}

fn split_host_port(endpoint: &str) -> anyhow::Result<(&str, &str)> {
    if let Some(rest) = endpoint.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .with_context(|| format!("invalid endpoint `{endpoint}`; missing `]`"))?;
        let port = suffix
            .strip_prefix(':')
            .with_context(|| format!("invalid endpoint `{endpoint}`; expected [host]:port"))?;
        return Ok((host, port));
    }

    endpoint
        .rsplit_once(':')
        .with_context(|| format!("invalid endpoint `{endpoint}`; expected <port> or <host>:<port>"))
}

fn parse_port(value: &str) -> anyhow::Result<u16> {
    value
        .parse::<u16>()
        .map_err(|err| anyhow!("invalid port `{value}`: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{ensure_nonzero_connect_endpoint, normalize_endpoint};

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
}
