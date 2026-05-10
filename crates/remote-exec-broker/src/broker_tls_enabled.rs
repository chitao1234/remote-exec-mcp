use std::sync::{Arc, OnceLock};

use anyhow::Context;
use reqwest::Identity;
use reqwest::tls::Certificate;
use rustls::RootCertStore;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::config::TargetConfig;

pub(crate) fn install_crypto_provider() -> anyhow::Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();

    INIT.get_or_init(|| {
        if rustls::crypto::CryptoProvider::get_default().is_some() {
            return Ok(());
        }

        let provider = rustls::crypto::ring::default_provider();
        match provider.install_default() {
            Ok(()) => Ok(()),
            Err(_) if rustls::crypto::CryptoProvider::get_default().is_some() => Ok(()),
            Err(_) => Err("failed to install rustls ring crypto provider".to_string()),
        }
    })
    .as_ref()
    .map(|_| ())
    .map_err(|message| anyhow::anyhow!(message.clone()))
}

pub(crate) fn ensure_https_target_supported(_: &str) -> anyhow::Result<()> {
    Ok(())
}

pub(crate) fn ensure_broker_url_supported(_: &str) -> anyhow::Result<()> {
    Ok(())
}

pub(crate) async fn build_daemon_https_client(
    config: &TargetConfig,
) -> anyhow::Result<reqwest::Client> {
    let ca_pem = config
        .ca_pem
        .as_ref()
        .context("ca_pem is required for https targets")?;
    let client_cert_pem = config
        .client_cert_pem
        .as_ref()
        .context("client_cert_pem is required for https targets")?;
    let client_key_pem = config
        .client_key_pem
        .as_ref()
        .context("client_key_pem is required for https targets")?;
    let (ca_pem, client_cert_pem, client_key_pem) = tokio::try_join!(
        tokio::fs::read(ca_pem),
        tokio::fs::read(client_cert_pem),
        tokio::fs::read(client_key_pem),
    )?;

    if config.pinned_server_cert_pem.is_none() {
        let ca = Certificate::from_pem(&ca_pem)?;
        let identity =
            Identity::from_pem(&[client_cert_pem.as_slice(), client_key_pem.as_slice()].concat())?;
        let mut builder = reqwest::Client::builder()
            .use_rustls_tls()
            .tls_certs_only([ca])
            .identity(identity);
        if config.skip_server_name_verification {
            builder = builder.danger_accept_invalid_hostnames(true);
        }
        return builder.build().context("building daemon client");
    }

    build_pinned_https_client(config, &ca_pem, &client_cert_pem, &client_key_pem).await
}

async fn build_pinned_https_client(
    config: &TargetConfig,
    ca_pem: &[u8],
    client_cert_pem: &[u8],
    client_key_pem: &[u8],
) -> anyhow::Result<reqwest::Client> {
    let roots = load_roots(ca_pem)?;
    let pinned_server_cert_pem = tokio::fs::read(
        config
            .pinned_server_cert_pem
            .as_ref()
            .context("pinned_server_cert_pem is required when pinning is enabled")?,
    )
    .await?;
    let verifier = Arc::new(PinnedServerCertificateVerifier::new(
        roots,
        config.skip_server_name_verification,
        load_pinned_certs(&pinned_server_cert_pem)?,
    )?);
    let cert_chain = load_certs(client_cert_pem)?;
    let key = load_key(client_key_pem)?;
    let tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(cert_chain, key)?;

    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .build()
        .context("building pinned daemon client")
}

#[derive(Debug)]
enum NameVerificationMode {
    Strict(Arc<WebPkiServerVerifier>),
    IgnoreName { roots: Arc<RootCertStore> },
}

struct PinnedServerCertificateVerifier {
    mode: NameVerificationMode,
    signature_algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
    pinned_leaf_certs: Vec<Vec<u8>>,
}

impl std::fmt::Debug for PinnedServerCertificateVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedServerCertificateVerifier")
            .field("mode", &self.mode)
            .field("pinned_leaf_cert_count", &self.pinned_leaf_certs.len())
            .finish()
    }
}

impl PinnedServerCertificateVerifier {
    fn new(
        roots: RootCertStore,
        skip_server_name_verification: bool,
        pinned_leaf_certs: Vec<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !pinned_leaf_certs.is_empty(),
            "pinned_server_cert_pem must contain at least one certificate"
        );

        let provider = rustls::crypto::CryptoProvider::get_default()
            .context("failed to load rustls crypto provider")?;
        let signature_algorithms = provider.signature_verification_algorithms;
        let mode = if skip_server_name_verification {
            NameVerificationMode::IgnoreName {
                roots: Arc::new(roots),
            }
        } else {
            NameVerificationMode::Strict(
                WebPkiServerVerifier::builder(Arc::new(roots))
                    .build()
                    .context("building strict server certificate verifier")?,
            )
        };

        Ok(Self {
            mode,
            signature_algorithms,
            pinned_leaf_certs,
        })
    }
}

impl ServerCertVerifier for PinnedServerCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.mode {
            NameVerificationMode::Strict(inner) => {
                inner.verify_server_cert(
                    end_entity,
                    intermediates,
                    server_name,
                    ocsp_response,
                    now,
                )?;
            }
            NameVerificationMode::IgnoreName { roots } => {
                let cert = ParsedCertificate::try_from(end_entity)?;
                rustls::client::verify_server_cert_signed_by_trust_anchor(
                    &cert,
                    roots,
                    intermediates,
                    now,
                    self.signature_algorithms.all,
                )?;
            }
        }

        if !self
            .pinned_leaf_certs
            .iter()
            .any(|pinned| pinned.as_slice() == end_entity.as_ref())
        {
            return Err(rustls::Error::General(
                "pinned server certificate mismatch".to_string(),
            ));
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.signature_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.signature_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.signature_algorithms.supported_schemes()
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
