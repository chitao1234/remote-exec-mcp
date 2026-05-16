use crate::config::TargetConfig;

pub(crate) const BROKER_TLS_FEATURE_REQUIRED_MESSAGE: &str =
    "https:// support requires the remote-exec-broker `broker-tls` Cargo feature";

pub(crate) async fn build_daemon_https_client(_: &TargetConfig) -> anyhow::Result<reqwest::Client> {
    anyhow::bail!(BROKER_TLS_FEATURE_REQUIRED_MESSAGE);
}

pub(crate) fn ensure_broker_url_supported(url: &str) -> anyhow::Result<()> {
    if url.starts_with("https://") {
        anyhow::bail!("broker URL `{url}` uses https://; {BROKER_TLS_FEATURE_REQUIRED_MESSAGE}");
    }
    Ok(())
}

pub(crate) fn install_crypto_provider() -> anyhow::Result<()> {
    Ok(())
}
