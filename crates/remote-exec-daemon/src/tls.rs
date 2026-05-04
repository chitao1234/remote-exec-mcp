use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;

use crate::config::{DaemonConfig, DaemonTransport};

#[allow(
    dead_code,
    reason = "Referenced when TLS feature-gated paths reject configuration"
)]
pub(crate) const FEATURE_REQUIRED_MESSAGE: &str =
    "transport = \"tls\" requires the remote-exec-daemon `tls` Cargo feature";

#[cfg(feature = "tls")]
#[path = "tls_enabled.rs"]
mod tls_impl;

#[cfg(not(feature = "tls"))]
#[path = "tls_disabled.rs"]
mod tls_impl;

pub(crate) use tls_impl::install_crypto_provider;
pub use tls_impl::{serve_tls, serve_tls_with_shutdown};

pub(crate) fn validate_config(config: &DaemonConfig) -> anyhow::Result<()> {
    if matches!(config.transport, DaemonTransport::Http)
        && config
            .tls
            .as_ref()
            .is_some_and(|tls| tls.pinned_client_cert_pem.is_some())
    {
        anyhow::bail!("pinned_client_cert_pem requires transport = \"tls\"");
    }

    tls_impl::validate_config(config)
}

pub async fn serve(app: Router, daemon_config: Arc<DaemonConfig>) -> anyhow::Result<()> {
    serve_with_shutdown(app, daemon_config, std::future::pending::<()>()).await
}

pub async fn serve_with_shutdown<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    match daemon_config.transport {
        DaemonTransport::Tls => serve_tls_with_shutdown(app, daemon_config, shutdown).await,
        DaemonTransport::Http => serve_http_with_shutdown(app, daemon_config, shutdown).await,
    }
}

pub async fn serve_http(app: Router, daemon_config: Arc<DaemonConfig>) -> anyhow::Result<()> {
    serve_http_with_shutdown(app, daemon_config, std::future::pending::<()>()).await
}

pub async fn serve_http_with_shutdown<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let listener = bind_listener(daemon_config.listen)?;
    tracing::info!(listen = %daemon_config.listen, "daemon http listener bound");
    let mut connections = JoinSet::new();
    let (connection_shutdown_tx, _) = watch::channel(());
    tokio::pin!(shutdown);

    loop {
        while let Some(result) = connections.try_join_next() {
            if let Err(err) = result {
                tracing::warn!(?err, "connection task failed");
            }
        }

        tokio::select! {
            _ = &mut shutdown => {
                break;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                tracing::debug!(peer = %peer_addr, "accepted tcp connection");
                let app = app.clone();
                let mut connection_shutdown = connection_shutdown_tx.subscribe();
                connections.spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(move |request: Request<Incoming>| {
                        let app = app.clone();
                        async move { app.oneshot(request.map(Body::new)).await }
                    });
                    let connection = http1::Builder::new().serve_connection(io, service);
                    tokio::pin!(connection);

                    tokio::select! {
                        result = &mut connection => {
                            if let Err(err) = result {
                                tracing::warn!(peer = %peer_addr, ?err, "http serve failed");
                            }
                        }
                        changed = connection_shutdown.changed() => {
                            if changed.is_ok() {
                                connection.as_mut().graceful_shutdown();
                            }
                            if let Err(err) = connection.await {
                                tracing::warn!(peer = %peer_addr, ?err, "http serve failed during shutdown");
                            }
                        }
                    }
                });
            }
        }
    }

    drop(listener);
    let _ = connection_shutdown_tx.send(());

    while let Some(result) = connections.join_next().await {
        if let Err(err) = result {
            tracing::warn!(?err, "connection task failed during shutdown");
        }
    }

    tracing::info!("daemon http listener stopped");
    Ok(())
}

pub(crate) fn bind_listener(addr: std::net::SocketAddr) -> std::io::Result<TcpListener> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };

    // Windows rebinding is stricter than Unix and the integration restart path
    // needs an explicit reuse policy to reacquire the configured port promptly.
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
}
