#[cfg(feature = "broker-tls")]
#[path = "broker_tls_enabled.rs"]
mod implementation;

#[cfg(not(feature = "broker-tls"))]
#[path = "broker_tls_disabled.rs"]
mod implementation;

pub(crate) use implementation::{
    build_daemon_https_client, ensure_broker_url_supported, ensure_https_target_supported,
    install_crypto_provider,
};

#[cfg(test)]
mod tests {
    #[test]
    fn broker_http_urls_remain_supported() {
        super::ensure_broker_url_supported("http://127.0.0.1:8787/mcp").unwrap();
    }

    #[cfg(feature = "broker-tls")]
    #[test]
    fn broker_https_urls_are_supported_when_feature_enabled() {
        super::ensure_broker_url_supported("https://broker.example.com/mcp").unwrap();
    }

    #[cfg(not(feature = "broker-tls"))]
    #[test]
    fn broker_https_urls_are_rejected_when_feature_disabled() {
        let err = super::ensure_broker_url_supported("https://broker.example.com/mcp").unwrap_err();
        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "broker-tls"))]
    #[test]
    fn https_targets_are_rejected_when_feature_disabled() {
        let err = super::ensure_https_target_supported("builder-a").unwrap_err();
        assert!(
            err.to_string().contains(
                "target `builder-a` uses https://; https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn build_daemon_https_client_is_rejected_when_feature_disabled() {
        let config: crate::config::TargetConfig = toml::from_str(
            r#"
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/client.pem"
client_key_pem = "/tmp/client.key"
"#,
        )
        .unwrap();

        let err = super::build_daemon_https_client(&config).await.unwrap_err();
        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }
}
