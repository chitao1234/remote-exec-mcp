use std::future::Future;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use crate::AppState;

pub async fn serve_tls(app: Router, state: Arc<AppState>) -> anyhow::Result<()> {
    serve_tls_with_shutdown(app, state, std::future::pending::<()>()).await
}

pub async fn serve_tls_with_shutdown<F>(
    app: Router,
    state: Arc<AppState>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let listener = bind_listener(state.config.listen)?;
    let tls = TlsAcceptor::from(Arc::new(server_config(&state)?));
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
                let (stream, _) = accepted?;
                let tls = tls.clone();
                let app = app.clone();
                let mut connection_shutdown = connection_shutdown_tx.subscribe();
                connections.spawn(async move {
                    let stream = match tls.accept(stream).await {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(?err, "tls accept failed");
                            return;
                        }
                    };

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
                                tracing::warn!(?err, "http serve failed");
                            }
                        }
                        changed = connection_shutdown.changed() => {
                            if changed.is_ok() {
                                connection.as_mut().graceful_shutdown();
                            }
                            if let Err(err) = connection.await {
                                tracing::warn!(?err, "http serve failed during shutdown");
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

    Ok(())
}

fn bind_listener(addr: std::net::SocketAddr) -> std::io::Result<TcpListener> {
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

fn server_config(state: &AppState) -> anyhow::Result<ServerConfig> {
    let certs = load_certs(&state.config.tls.cert_pem)?;
    let key = load_key(&state.config.tls.key_pem)?;
    let client_roots = load_roots(&state.config.tls.ca_pem)?;
    let verifier = WebPkiClientVerifier::builder(Arc::new(client_roots)).build()?;

    Ok(ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?)
}

fn load_certs(path: &std::path::Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let mut pem = std::io::BufReader::new(std::fs::File::open(path)?);
    Ok(rustls_pemfile::certs(&mut pem).collect::<Result<Vec<_>, _>>()?)
}

fn load_key(path: &std::path::Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut pem = std::io::BufReader::new(std::fs::File::open(path)?);
    rustls_pemfile::private_key(&mut pem)?.context("missing private key")
}

fn load_roots(path: &std::path::Path) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in load_certs(path)? {
        roots.add(cert)?;
    }
    Ok(roots)
}
