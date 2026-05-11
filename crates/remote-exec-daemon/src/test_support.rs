use std::future::Future;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

use crate::config::DaemonConfig;

pub async fn run_until_on_listener<F>(
    config: DaemonConfig,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    crate::run_until_on_bound_listener(Arc::new(config), listener, shutdown).await
}

pub async fn serve_tls_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    crate::tls::serve_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
}

pub async fn serve_http_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    crate::tls::serve_http_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
}
