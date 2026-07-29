use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use remote_exec_proto::reverse::{
    MAX_REVERSE_REGISTRATION_BYTES, REVERSE_PROTOCOL_MAGIC, REVERSE_PROTOCOL_VERSION,
    ReverseRegistration, ReverseRegistrationAck,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::config::{BrokerConfig, ReverseListenerConfig, ReverseTransport};

pub(crate) trait ReverseIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> ReverseIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
type BoxIo = Box<dyn ReverseIo>;

struct ReverseLane {
    io: BoxIo,
    daemon_instance_id: String,
}

struct LanePool {
    lanes: Mutex<VecDeque<ReverseLane>>,
    notify: Notify,
    closed: AtomicBool,
}

impl LanePool {
    fn new() -> Self {
        Self {
            lanes: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    async fn insert(&self, lane: ReverseLane) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.closed.load(Ordering::Acquire),
            "reverse lane pool is closed"
        );
        self.lanes.lock().await.push_back(lane);
        self.notify.notify_one();
        Ok(())
    }

    async fn take(&self, timeout: Duration) -> anyhow::Result<ReverseLane> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(lane) = self.lanes.lock().await.pop_front() {
                    return Ok(lane);
                }
                anyhow::ensure!(
                    !self.closed.load(Ordering::Acquire),
                    "reverse lane pool is closed"
                );
                self.notify.notified().await;
            }
        })
        .await
        .context("timed out waiting for an available reverse lane")?
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

struct ReverseTarget {
    pool: Arc<LanePool>,
    bearer_token: Option<String>,
    base_url: String,
}

struct ReverseTransportInner {
    targets: BTreeMap<String, ReverseTarget>,
    cancel: CancellationToken,
}

#[derive(Clone)]
pub(crate) struct ReverseTransportManager {
    inner: Arc<ReverseTransportInner>,
}

impl Drop for ReverseTransportInner {
    fn drop(&mut self) {
        self.cancel.cancel();
        for target in self.targets.values() {
            target.pool.close();
        }
    }
}

impl ReverseTransportManager {
    pub(crate) async fn start(config: &BrokerConfig) -> anyhow::Result<Option<Self>> {
        let reverse_targets = config
            .targets
            .iter()
            .filter(|(_, target)| target.is_reverse())
            .collect::<Vec<_>>();
        if reverse_targets.is_empty() {
            return Ok(None);
        }
        let listener_config = config
            .reverse
            .as_ref()
            .context("reverse listener configuration is missing")?
            .clone();
        let cancel = CancellationToken::new();
        let mut targets = BTreeMap::new();

        for (name, target_config) in reverse_targets {
            let pool = Arc::new(LanePool::new());
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let lane_timeout = Duration::from_millis(listener_config.lane_wait_timeout_ms);
            tokio::spawn(serve_bridge(
                name.clone(),
                listener,
                pool.clone(),
                lane_timeout,
                cancel.clone(),
            ));
            targets.insert(
                name.clone(),
                ReverseTarget {
                    pool,
                    bearer_token: target_config
                        .http_auth
                        .as_ref()
                        .map(|auth| auth.bearer_token.clone()),
                    base_url: format!("http://{addr}"),
                },
            );
        }

        let inner = Arc::new(ReverseTransportInner { targets, cancel });
        let manager = Self { inner };
        manager.spawn_acceptor(listener_config).await?;
        Ok(Some(manager))
    }

    pub(crate) fn base_url(&self, target: &str) -> Option<&str> {
        self.inner
            .targets
            .get(target)
            .map(|target| target.base_url.as_str())
    }

    async fn spawn_acceptor(&self, config: ReverseListenerConfig) -> anyhow::Result<()> {
        let listener = TcpListener::bind(config.listen).await?;
        let local_addr = listener.local_addr()?;
        tracing::info!(listen = %local_addr, transport = ?config.transport, "broker reverse listener bound");
        let acceptor = build_acceptor(&config).await?;
        let inner = Arc::downgrade(&self.inner);
        let cancel = self.inner.cancel.clone();
        let limit = Arc::new(Semaphore::new(config.max_connections));
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = cancel.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                let (stream, peer) = match accepted {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::warn!(error = %err, "reverse listener accept failed");
                        continue;
                    }
                };
                let permit = match limit.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        tracing::warn!(peer = %peer, "rejecting reverse lane: connection limit reached");
                        continue;
                    }
                };
                let Some(inner) = inner.upgrade() else {
                    break;
                };
                let acceptor = acceptor.clone();
                let timeout = Duration::from_millis(config.registration_timeout_ms);
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(err) = accept_reverse_lane(inner, acceptor, stream, timeout).await {
                        tracing::warn!(peer = %peer, error = %err, "reverse lane registration failed");
                    }
                });
            }
        });
        Ok(())
    }
}

async fn serve_bridge(
    target: String,
    listener: TcpListener,
    pool: Arc<LanePool>,
    lane_timeout: Duration,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (mut local, _) = match accepted {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(target = %target, error = %err, "reverse bridge accept failed");
                continue;
            }
        };
        let pool = pool.clone();
        let target = target.clone();
        tokio::spawn(async move {
            match pool.take(lane_timeout).await {
                Ok(mut lane) => {
                    tracing::debug!(target = %target, daemon_instance_id = %lane.daemon_instance_id, "paired reverse lane");
                    if let Err(err) = tokio::io::copy_bidirectional(&mut local, &mut lane.io).await
                    {
                        tracing::debug!(target = %target, error = %err, "reverse bridge connection ended with error");
                    }
                }
                Err(err) => {
                    tracing::warn!(target = %target, error = %err, "reverse bridge could not acquire lane")
                }
            }
        });
    }
}

#[derive(Clone)]
enum ReverseAcceptor {
    Plain,
    #[cfg(feature = "broker-tls")]
    Tls(tokio_rustls::TlsAcceptor),
}

async fn build_acceptor(config: &ReverseListenerConfig) -> anyhow::Result<ReverseAcceptor> {
    match config.transport {
        ReverseTransport::Http => Ok(ReverseAcceptor::Plain),
        ReverseTransport::Tls => build_tls_acceptor(config).await,
    }
}

#[cfg(feature = "broker-tls")]
async fn build_tls_acceptor(config: &ReverseListenerConfig) -> anyhow::Result<ReverseAcceptor> {
    use rustls::RootCertStore;
    use rustls::server::WebPkiClientVerifier;

    let tls = config
        .tls
        .as_ref()
        .context("reverse TLS config is missing")?;
    let (cert_pem, key_pem, ca_pem) = tokio::try_join!(
        tokio::fs::read(&tls.cert_pem),
        tokio::fs::read(&tls.key_pem),
        tokio::fs::read(&tls.ca_pem),
    )?;
    let mut cert_reader = cert_pem.as_slice();
    let certs = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;
    let mut key_reader = key_pem.as_slice();
    let key = rustls_pemfile::private_key(&mut key_reader)?.context("missing reverse TLS key")?;
    let mut roots = RootCertStore::empty();
    let mut ca_reader = ca_pem.as_slice();
    for cert in rustls_pemfile::certs(&mut ca_reader) {
        roots.add(cert?)?;
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;
    let server = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?;
    Ok(ReverseAcceptor::Tls(tokio_rustls::TlsAcceptor::from(
        Arc::new(server),
    )))
}

#[cfg(not(feature = "broker-tls"))]
async fn build_tls_acceptor(_: &ReverseListenerConfig) -> anyhow::Result<ReverseAcceptor> {
    anyhow::bail!("TLS reverse listener requires the remote-exec-broker `broker-tls` feature")
}

async fn accept_reverse_lane(
    inner: Arc<ReverseTransportInner>,
    acceptor: ReverseAcceptor,
    stream: TcpStream,
    timeout: Duration,
) -> anyhow::Result<()> {
    let (mut io, authenticated_target): (BoxIo, Option<String>) = match acceptor {
        ReverseAcceptor::Plain => (Box::new(stream), None),
        #[cfg(feature = "broker-tls")]
        ReverseAcceptor::Tls(acceptor) => {
            let tls = acceptor.accept(stream).await?;
            let target = reverse_tls_client_common_name(&tls)?;
            (Box::new(tls), Some(target))
        }
    };
    let registration = tokio::time::timeout(timeout, read_registration(&mut io)).await??;
    anyhow::ensure!(
        registration.protocol_version == REVERSE_PROTOCOL_VERSION,
        "unsupported reverse protocol version {}",
        registration.protocol_version
    );
    let target = inner
        .targets
        .get(&registration.target)
        .context("reverse lane registered an unknown target")?;
    if let Some(authenticated_target) = authenticated_target {
        anyhow::ensure!(
            authenticated_target == registration.target,
            "reverse TLS client certificate identity does not match registered target"
        );
    }
    anyhow::ensure!(
        target.bearer_token == registration.bearer_token,
        "reverse lane authentication failed"
    );
    write_ack(
        &mut io,
        &ReverseRegistrationAck {
            accepted: true,
            message: "accepted".to_string(),
        },
    )
    .await?;
    target
        .pool
        .insert(ReverseLane {
            io,
            daemon_instance_id: registration.daemon_instance_id,
        })
        .await
}

#[cfg(feature = "broker-tls")]
fn reverse_tls_client_common_name(
    stream: &tokio_rustls::server::TlsStream<TcpStream>,
) -> anyhow::Result<String> {
    use x509_parser::prelude::FromDer;

    let cert = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .context("reverse TLS client did not present a certificate")?;
    let (_, parsed) = x509_parser::certificate::X509Certificate::from_der(cert.as_ref())
        .map_err(|err| anyhow::anyhow!("parsing reverse TLS client certificate: {err}"))?;
    parsed
        .subject()
        .iter_common_name()
        .next()
        .context("reverse TLS client certificate is missing a common name")?
        .as_str()
        .map(str::to_string)
        .map_err(anyhow::Error::from)
}

async fn read_registration(io: &mut BoxIo) -> anyhow::Result<ReverseRegistration> {
    let mut magic = [0u8; REVERSE_PROTOCOL_MAGIC.len()];
    io.read_exact(&mut magic).await?;
    anyhow::ensure!(
        &magic == REVERSE_PROTOCOL_MAGIC,
        "invalid reverse protocol magic"
    );
    let length = io.read_u32().await? as usize;
    anyhow::ensure!(
        length <= MAX_REVERSE_REGISTRATION_BYTES,
        "reverse registration exceeds size limit"
    );
    let mut payload = vec![0u8; length];
    io.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

async fn write_ack(io: &mut BoxIo, ack: &ReverseRegistrationAck) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(ack)?;
    io.write_all(REVERSE_PROTOCOL_MAGIC).await?;
    io.write_u32(payload.len() as u32).await?;
    io.write_all(&payload).await?;
    io.flush().await?;
    Ok(())
}
