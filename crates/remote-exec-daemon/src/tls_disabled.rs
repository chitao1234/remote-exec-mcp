use std::future::Future;
use std::sync::Arc;

use axum::Router;

use crate::config::{DaemonConfig, DaemonTransport};

pub(crate) fn install_crypto_provider() {}

pub(crate) fn validate_config(config: &DaemonConfig) -> anyhow::Result<()> {
    if matches!(config.transport, DaemonTransport::Tls) {
        anyhow::bail!(super::FEATURE_REQUIRED_MESSAGE);
    }

    Ok(())
}

pub async fn serve_tls(_: Router, _: Arc<DaemonConfig>) -> anyhow::Result<()> {
    anyhow::bail!(super::FEATURE_REQUIRED_MESSAGE);
}

pub async fn serve_tls_with_shutdown<F>(_: Router, _: Arc<DaemonConfig>, _: F) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    anyhow::bail!(super::FEATURE_REQUIRED_MESSAGE);
}
