use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use crate::AppState;

pub async fn serve_tls(app: Router, state: Arc<AppState>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(state.config.listen).await?;
    let tls = TlsAcceptor::from(Arc::new(server_config(&state)?));

    loop {
        let (stream, _) = listener.accept().await?;
        let tls = tls.clone();
        let app = app.clone();
        tokio::spawn(async move {
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

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                tracing::warn!(?err, "http serve failed");
            }
        });
    }
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
    rustls_pemfile::private_key(&mut pem)?
        .context("missing private key")
}

fn load_roots(path: &std::path::Path) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in load_certs(path)? {
        roots.add(cert)?;
    }
    Ok(roots)
}
