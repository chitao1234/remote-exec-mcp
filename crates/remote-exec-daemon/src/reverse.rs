use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use remote_exec_proto::reverse::{
    MAX_REVERSE_REGISTRATION_BYTES, REVERSE_PROTOCOL_MAGIC, ReverseRegistration,
    ReverseRegistrationAck,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::AppState;
use crate::config::{DaemonConfig, DaemonTransport, ReverseConnectionConfig};

trait ReverseIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> ReverseIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
type BoxIo = Box<dyn ReverseIo>;

#[derive(Debug)]
enum LaneEvent {
    Connected(usize),
    Busy(usize),
    Disconnected(usize),
}

pub(crate) async fn serve<F>(
    state: AppState,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let reverse = daemon_config
        .reverse
        .as_ref()
        .context("reverse configuration is missing")?
        .clone();
    let app = crate::http::routes::router(Arc::new(state.clone()), daemon_config.clone());
    let daemon_instance_id = Arc::new(state.daemon_instance_id().to_string());
    let cancel = CancellationToken::new();
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mut idle = BTreeSet::new();
    let mut workers = 0usize;

    while workers < reverse.min_idle_connections {
        spawn_lane_worker(
            workers,
            app.clone(),
            daemon_config.clone(),
            daemon_instance_id.clone(),
            reverse.clone(),
            events_tx.clone(),
            cancel.clone(),
        );
        workers += 1;
    }

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            event = events_rx.recv() => {
                let Some(event) = event else { break };
                match event {
                    LaneEvent::Connected(id) => { idle.insert(id); }
                    LaneEvent::Busy(id) | LaneEvent::Disconnected(id) => { idle.remove(&id); }
                }
                while idle.len() < reverse.min_idle_connections && workers < reverse.max_connections {
                    spawn_lane_worker(
                        workers,
                        app.clone(),
                        daemon_config.clone(),
                        daemon_instance_id.clone(),
                        reverse.clone(),
                        events_tx.clone(),
                        cancel.clone(),
                    );
                    workers += 1;
                }
            }
        }
    }

    cancel.cancel();
    state.join_background_tasks().await;
    Ok(())
}

fn spawn_lane_worker(
    id: usize,
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    daemon_instance_id: Arc<String>,
    reverse: ReverseConnectionConfig,
    events: mpsc::UnboundedSender<LaneEvent>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(reverse.reconnect_min_ms);
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let result = run_lane(
                id,
                app.clone(),
                daemon_config.clone(),
                daemon_instance_id.as_str(),
                &reverse,
                &events,
                &cancel,
            )
            .await;
            let _ = events.send(LaneEvent::Disconnected(id));
            if cancel.is_cancelled() {
                break;
            }
            if let Err(err) = result {
                tracing::debug!(target = %daemon_config.target, lane_id = id, error = %err, "reverse lane disconnected");
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = backoff
                .saturating_mul(2)
                .min(Duration::from_millis(reverse.reconnect_max_ms));
        }
    });
}

async fn run_lane(
    id: usize,
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    daemon_instance_id: &str,
    reverse: &ReverseConnectionConfig,
    events: &mpsc::UnboundedSender<LaneEvent>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let stream = TcpStream::connect(&reverse.broker_addr).await?;
    stream.set_nodelay(true)?;
    let mut io = connect_transport(stream, reverse).await?;
    register_lane(&mut io, &daemon_config, daemon_instance_id, reverse).await?;
    let _ = events.send(LaneEvent::Connected(id));
    tracing::debug!(target = %daemon_config.target, lane_id = id, "reverse lane registered");

    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let service = service_fn(move |request: Request<Incoming>| {
        let app = app.clone();
        let events = events.clone();
        let busy = busy.clone();
        async move {
            if !busy.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let _ = events.send(LaneEvent::Busy(id));
            }
            app.oneshot(request.map(Body::new)).await
        }
    });
    let connection = http1::Builder::new()
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        result = &mut connection => result.context("serving reverse HTTP connection")?,
        _ = cancel.cancelled() => {
            connection.as_mut().graceful_shutdown();
            connection.await.context("shutting down reverse HTTP connection")?;
        }
    }
    Ok(())
}

async fn register_lane(
    io: &mut BoxIo,
    daemon_config: &DaemonConfig,
    daemon_instance_id: &str,
    reverse: &ReverseConnectionConfig,
) -> anyhow::Result<()> {
    let registration = ReverseRegistration::new(
        daemon_config.target.clone(),
        daemon_instance_id.to_string(),
        reverse.bearer_token.clone(),
    );
    let payload = serde_json::to_vec(&registration)?;
    anyhow::ensure!(
        payload.len() <= MAX_REVERSE_REGISTRATION_BYTES,
        "reverse registration exceeds size limit"
    );
    io.write_all(REVERSE_PROTOCOL_MAGIC).await?;
    io.write_u32(payload.len() as u32).await?;
    io.write_all(&payload).await?;
    io.flush().await?;

    let mut magic = [0u8; REVERSE_PROTOCOL_MAGIC.len()];
    tokio::time::timeout(
        Duration::from_millis(reverse.registration_timeout_ms),
        async {
            io.read_exact(&mut magic).await?;
            anyhow::ensure!(
                &magic == REVERSE_PROTOCOL_MAGIC,
                "invalid reverse acknowledgement magic"
            );
            let length = io.read_u32().await? as usize;
            anyhow::ensure!(
                length <= MAX_REVERSE_REGISTRATION_BYTES,
                "reverse acknowledgement exceeds size limit"
            );
            let mut payload = vec![0u8; length];
            io.read_exact(&mut payload).await?;
            let ack: ReverseRegistrationAck = serde_json::from_slice(&payload)?;
            anyhow::ensure!(
                ack.accepted,
                "broker rejected reverse lane: {}",
                ack.message
            );
            Ok::<_, anyhow::Error>(())
        },
    )
    .await
    .context("reverse registration timed out")??;
    Ok(())
}

async fn connect_transport(
    stream: TcpStream,
    reverse: &ReverseConnectionConfig,
) -> anyhow::Result<BoxIo> {
    match reverse.transport {
        DaemonTransport::Http => Ok(Box::new(stream)),
        DaemonTransport::Tls => connect_tls(stream, reverse).await,
    }
}

#[cfg(feature = "tls")]
async fn connect_tls(
    stream: TcpStream,
    reverse: &ReverseConnectionConfig,
) -> anyhow::Result<BoxIo> {
    use rustls::RootCertStore;
    use rustls::pki_types::ServerName;

    let tls = reverse
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
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)?;
    let server_name = ServerName::try_from(tls.server_name.clone())?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
    Ok(Box::new(connector.connect(server_name, stream).await?))
}

#[cfg(not(feature = "tls"))]
async fn connect_tls(_: TcpStream, _: &ReverseConnectionConfig) -> anyhow::Result<BoxIo> {
    anyhow::bail!("TLS reverse transport requires the remote-exec-daemon `tls` feature")
}
