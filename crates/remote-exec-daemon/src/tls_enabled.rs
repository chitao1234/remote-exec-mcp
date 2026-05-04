use std::future::Future;
use std::sync::Arc;
use std::sync::Once;

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ParsedCertificate, WebPkiClientVerifier};
use rustls::{
    DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme,
};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use crate::config::{DaemonConfig, DaemonTransport};

pub(crate) fn install_crypto_provider() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let provider = rustls::crypto::ring::default_provider();
        let _ = provider.install_default();
    });
}

pub(crate) fn validate_config(config: &DaemonConfig) -> anyhow::Result<()> {
    if matches!(config.transport, DaemonTransport::Tls) {
        anyhow::ensure!(
            config.tls.is_some(),
            "tls config is required when transport = \"tls\""
        );
    }

    Ok(())
}

pub async fn serve_tls(app: Router, daemon_config: Arc<DaemonConfig>) -> anyhow::Result<()> {
    serve_tls_with_shutdown(app, daemon_config, std::future::pending::<()>()).await
}

pub async fn serve_tls_with_shutdown<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let listener = super::bind_listener(daemon_config.listen)?;
    tracing::info!(listen = %daemon_config.listen, "daemon tls listener bound");
    let tls = TlsAcceptor::from(Arc::new(server_config(daemon_config.as_ref()).await?));
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
                let tls = tls.clone();
                let app = app.clone();
                let mut connection_shutdown = connection_shutdown_tx.subscribe();
                connections.spawn(async move {
                    let stream = match tls.accept(stream).await {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(peer = %peer_addr, ?err, "tls accept failed");
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

    tracing::info!("daemon tls listener stopped");
    Ok(())
}

async fn server_config(daemon_config: &DaemonConfig) -> anyhow::Result<ServerConfig> {
    let tls = daemon_config
        .tls
        .as_ref()
        .context("tls config is required when transport = \"tls\"")?;
    let (cert_pem, key_pem, ca_pem) = tokio::try_join!(
        tokio::fs::read(&tls.cert_pem),
        tokio::fs::read(&tls.key_pem),
        tokio::fs::read(&tls.ca_pem),
    )?;
    let certs = load_certs(&cert_pem)?;
    let key = load_key(&key_pem)?;
    let client_roots = Arc::new(load_roots(&ca_pem)?);
    let inner = WebPkiClientVerifier::builder(client_roots).build()?;
    let verifier = if let Some(pinned_client_cert_pem) = &tls.pinned_client_cert_pem {
        let pinned_client_cert_pem = tokio::fs::read(pinned_client_cert_pem).await?;
        Arc::new(PinnedClientCertificateVerifier::new(
            inner,
            load_pinned_certs(&pinned_client_cert_pem)?,
        )?) as Arc<dyn ClientCertVerifier>
    } else {
        inner
    };

    Ok(ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?)
}

#[derive(Debug)]
struct PinnedClientCertificateVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    pinned_leaf_certs: Vec<Vec<u8>>,
}

impl PinnedClientCertificateVerifier {
    fn new(
        inner: Arc<dyn ClientCertVerifier>,
        pinned_leaf_certs: Vec<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !pinned_leaf_certs.is_empty(),
            "pinned_client_cert_pem must contain at least one certificate"
        );

        Ok(Self {
            inner,
            pinned_leaf_certs,
        })
    }
}

impl ClientCertVerifier for PinnedClientCertificateVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let _ = ParsedCertificate::try_from(end_entity)?;
        self.inner
            .verify_client_cert(end_entity, intermediates, now)?;

        if !self
            .pinned_leaf_certs
            .iter()
            .any(|pinned| pinned.as_slice() == end_entity.as_ref())
        {
            return Err(rustls::Error::General(
                "pinned client certificate mismatch".to_string(),
            ));
        }

        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn load_certs(pem_bytes: &[u8]) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let mut pem = std::io::Cursor::new(pem_bytes);
    Ok(rustls_pemfile::certs(&mut pem).collect::<Result<Vec<_>, _>>()?)
}

fn load_key(pem_bytes: &[u8]) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut pem = std::io::Cursor::new(pem_bytes);
    rustls_pemfile::private_key(&mut pem)?.context("missing private key")
}

fn load_roots(pem_bytes: &[u8]) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in load_certs(pem_bytes)? {
        roots.add(cert)?;
    }
    Ok(roots)
}

fn load_pinned_certs(pem_bytes: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
    Ok(load_certs(pem_bytes)?
        .into_iter()
        .map(|cert| cert.as_ref().to_vec())
        .collect())
}
