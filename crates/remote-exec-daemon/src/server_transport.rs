use std::future::Future;
use std::sync::Arc;

use axum::Router;
use tokio::net::{TcpListener, TcpSocket};

use crate::config::{DaemonConfig, DaemonTransport};
use crate::http_serve::{AcceptStream, AcceptedStream, serve_http1_connections};

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

pub(crate) async fn serve_with_shutdown_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    match daemon_config.transport {
        DaemonTransport::Tls => {
            crate::tls::serve_tls_with_shutdown_on_listener(app, daemon_config, listener, shutdown)
                .await
        }
        DaemonTransport::Http => {
            serve_http_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
        }
    }
}

pub(crate) async fn serve_http_with_shutdown_on_listener<F>(
    app: Router,
    _daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let local_addr = listener.local_addr()?;
    tracing::info!(listen = %local_addr, "daemon http listener bound");
    let accept_stream: AcceptStream =
        Arc::new(|stream| Box::pin(async move { Ok(Some(Box::new(stream) as AcceptedStream)) }));
    serve_http1_connections(listener, app, shutdown, accept_stream, "http").await
}
