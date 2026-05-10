use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use futures_util::future::BoxFuture;
use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;

pub trait AcceptedStreamIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}

impl<T> AcceptedStreamIo for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}

pub type AcceptedStream = Box<dyn AcceptedStreamIo>;
pub type AcceptStream = Arc<
    dyn Fn(TcpStream) -> BoxFuture<'static, anyhow::Result<Option<AcceptedStream>>> + Send + Sync,
>;

pub async fn serve_http1_connections<F>(
    listener: TcpListener,
    app: Router,
    shutdown: F,
    accept_stream: AcceptStream,
    log_label: &'static str,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
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
                tracing::debug!(
                    peer = %peer_addr,
                    transport = log_label,
                    "accepted tcp connection"
                );
                let app = app.clone();
                let accept_stream = accept_stream.clone();
                let mut connection_shutdown = connection_shutdown_tx.subscribe();
                connections.spawn(async move {
                    let stream = match accept_stream(stream).await {
                        Ok(Some(stream)) => stream,
                        Ok(None) => return,
                        Err(err) => {
                            tracing::warn!(
                                peer = %peer_addr,
                                ?err,
                                transport = log_label,
                                "connection accept failed"
                            );
                            return;
                        }
                    };

                    let io = TokioIo::new(stream);
                    let service = service_fn(move |request: Request<Incoming>| {
                        let app = app.clone();
                        async move { app.oneshot(request.map(Body::new)).await }
                    });
                    let connection = http1::Builder::new()
                        .serve_connection(io, service)
                        .with_upgrades();
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

    tracing::info!(transport = log_label, "daemon listener stopped");
    Ok(())
}
