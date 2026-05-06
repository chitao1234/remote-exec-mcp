use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use remote_exec_proto::port_forward::{
    ensure_nonzero_connect_endpoint, normalize_endpoint, udp_connector_endpoint,
};
use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, write_frame, write_preface};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortProtocol as PublicForwardPortProtocol, ForwardPortSpec,
    ForwardPortStatus,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::TargetHandle;
use crate::daemon_client::DaemonClientError;
use crate::local_port_backend::LocalPortClient;

const UDP_CONNECTOR_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const UDP_CONNECTOR_IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub enum SideHandle {
    Target { name: String, handle: TargetHandle },
    Local(LocalPortClient),
}

impl SideHandle {
    pub fn local() -> Self {
        Self::Local(LocalPortClient::global())
    }

    pub fn target(name: String, handle: TargetHandle) -> Self {
        Self::Target { name, handle }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Target { name, .. } => name,
            Self::Local(_) => "local",
        }
    }

    pub async fn port_tunnel(&self) -> Result<PortTunnel, DaemonClientError> {
        match self {
            Self::Target { handle, .. } => handle.port_tunnel().await,
            Self::Local(client) => PortTunnel::local(client.state()).await,
        }
    }
}

pub struct PortTunnel {
    tx: mpsc::Sender<Frame>,
    rx: Mutex<mpsc::Receiver<anyhow::Result<Frame>>>,
}

impl PortTunnel {
    pub fn from_stream<S>(stream: S) -> Result<Self, DaemonClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (tx, mut write_rx) = mpsc::channel::<Frame>(128);
        let (read_tx, read_rx) = mpsc::channel::<anyhow::Result<Frame>>(128);
        tokio::spawn(async move {
            while let Some(frame) = write_rx.recv().await {
                if let Err(err) = write_frame(&mut writer, &frame).await {
                    tracing::debug!(error = %err, "port tunnel writer stopped");
                    return;
                }
            }
        });
        tokio::spawn(async move {
            loop {
                match read_frame(&mut reader).await {
                    Ok(frame) => {
                        if read_tx.send(Ok(frame)).await.is_err() {
                            return;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                        let _ = read_tx
                            .send(Err(anyhow::anyhow!("port tunnel closed")))
                            .await;
                        return;
                    }
                    Err(err) => {
                        let _ = read_tx.send(Err(err.into())).await;
                        return;
                    }
                };
            }
        });
        Ok(Self {
            tx,
            rx: Mutex::new(read_rx),
        })
    }

    pub async fn local(
        state: Arc<remote_exec_host::HostRuntimeState>,
    ) -> Result<Self, DaemonClientError> {
        let (mut broker_side, daemon_side) = tokio::io::duplex(256 * 1024);
        tokio::spawn(remote_exec_host::port_forward::serve_tunnel(
            state,
            daemon_side,
        ));
        write_preface(&mut broker_side)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Self::from_stream(broker_side)
    }

    pub async fn send(&self, frame: Frame) -> anyhow::Result<()> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("port tunnel writer is closed"))
    }

    pub async fn recv(&self) -> anyhow::Result<Frame> {
        self.rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("port tunnel reader is closed"))?
    }

    pub async fn close_stream(&self, stream_id: u32) -> anyhow::Result<()> {
        self.send(Frame {
            frame_type: remote_exec_proto::port_tunnel::FrameType::Close,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await
    }
}

#[derive(Clone, Default)]
pub struct PortForwardStore {
    entries: Arc<RwLock<HashMap<String, PortForwardRecord>>>,
}

impl PortForwardStore {
    pub async fn insert(&self, record: PortForwardRecord) {
        self.entries
            .write()
            .await
            .insert(record.entry.forward_id.clone(), record);
    }

    pub async fn list(&self, filter: &PortForwardFilter) -> Vec<ForwardPortEntry> {
        let mut entries = self
            .entries
            .read()
            .await
            .values()
            .filter(|record| filter.matches(&record.entry))
            .map(|record| record.entry.clone())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.forward_id.cmp(&right.forward_id));
        entries
    }

    pub async fn close(&self, forward_ids: &[String]) -> anyhow::Result<Vec<PortForwardRecord>> {
        let mut entries = self.entries.write().await;
        for forward_id in forward_ids {
            anyhow::ensure!(
                entries.contains_key(forward_id),
                "unknown forward_id `{forward_id}`"
            );
        }
        Ok(forward_ids
            .iter()
            .filter_map(|forward_id| entries.remove(forward_id))
            .collect())
    }

    pub async fn mark_failed(&self, forward_id: &str, error: String) {
        let mut entries = self.entries.write().await;
        if let Some(record) = entries.get_mut(forward_id) {
            record.entry.status = ForwardPortStatus::Failed;
            record.entry.last_error = Some(error);
        }
    }

    pub async fn drain(&self) -> Vec<PortForwardRecord> {
        self.entries
            .write()
            .await
            .drain()
            .map(|(_, record)| record)
            .collect()
    }
}

pub struct PortForwardFilter {
    pub listen_side: Option<String>,
    pub connect_side: Option<String>,
    pub forward_ids: Vec<String>,
}

impl PortForwardFilter {
    fn matches(&self, entry: &ForwardPortEntry) -> bool {
        if let Some(listen_side) = &self.listen_side {
            if &entry.listen_side != listen_side {
                return false;
            }
        }
        if let Some(connect_side) = &self.connect_side {
            if &entry.connect_side != connect_side {
                return false;
            }
        }
        self.forward_ids.is_empty() || self.forward_ids.contains(&entry.forward_id)
    }
}

pub struct PortForwardRecord {
    pub entry: ForwardPortEntry,
    pub listener_stream_id: u32,
    pub listen_tunnel: Arc<PortTunnel>,
    pub cancel: CancellationToken,
}

pub struct OpenedForward {
    pub record: PortForwardRecord,
}

#[derive(Clone)]
struct ForwardRuntime {
    forward_id: String,
    listen_side: SideHandle,
    connect_side: SideHandle,
    protocol: PublicForwardPortProtocol,
    connect_endpoint: String,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
    cancel: CancellationToken,
}

#[derive(Debug, Deserialize, Serialize)]
struct EndpointMeta {
    endpoint: String,
}

#[derive(Debug, Deserialize)]
struct TcpAcceptMeta {
    listener_stream_id: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct UdpDatagramMeta {
    peer: String,
}

struct UdpPeerConnector {
    stream_id: u32,
    last_used: Instant,
}

pub async fn open_forward(
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    spec: &ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let listen_endpoint = normalize_endpoint(&spec.listen_endpoint)?;
    let connect_endpoint = ensure_nonzero_connect_endpoint(&spec.connect_endpoint)?;
    match spec.protocol {
        PublicForwardPortProtocol::Tcp => {
            open_tcp_forward(
                store,
                listen_side,
                connect_side,
                listen_endpoint,
                connect_endpoint,
                spec.clone(),
            )
            .await
        }
        PublicForwardPortProtocol::Udp => {
            open_udp_forward(
                store,
                listen_side,
                connect_side,
                listen_endpoint,
                connect_endpoint,
                spec.clone(),
            )
            .await
        }
    }
}

async fn open_tcp_forward(
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    listen_endpoint: String,
    connect_endpoint: String,
    spec: ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let listen_tunnel = Arc::new(
        listen_side
            .port_tunnel()
            .await
            .with_context(|| format!("opening port tunnel to `{}`", listen_side.name()))?,
    );
    let connect_tunnel = Arc::new(
        connect_side
            .port_tunnel()
            .await
            .with_context(|| format!("opening port tunnel to `{}`", connect_side.name()))?,
    );
    let listener_stream_id = 1;
    listen_tunnel
        .send(Frame {
            frame_type: FrameType::TcpListen,
            flags: 0,
            stream_id: listener_stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: listen_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| {
            format!(
                "opening tcp listener on `{}` at `{listen_endpoint}`",
                listen_side.name()
            )
        })?;
    let listen_response = loop {
        let frame = listen_tunnel.recv().await.with_context(|| {
            format!(
                "waiting for tcp listener on `{}` at `{listen_endpoint}`",
                listen_side.name()
            )
        })?;
        match frame.frame_type {
            FrameType::TcpListenOk if frame.stream_id == listener_stream_id => {
                break decode_tunnel_meta::<EndpointMeta>(&frame)?.endpoint;
            }
            FrameType::Error if frame.stream_id == listener_stream_id => {
                return Err(tunnel_error(&frame)).with_context(|| {
                    format!(
                        "opening tcp listener on `{}` at `{listen_endpoint}`",
                        listen_side.name()
                    )
                });
            }
            _ => {}
        }
    };

    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: PublicForwardPortProtocol::Tcp,
        connect_endpoint: connect_endpoint.clone(),
        listen_tunnel: listen_tunnel.clone(),
        connect_tunnel,
        cancel: cancel.clone(),
    };
    spawn_forward(runtime, store);

    Ok(OpenedForward {
        record: PortForwardRecord {
            entry: ForwardPortEntry {
                forward_id,
                listen_side: listen_side.name().to_string(),
                listen_endpoint: listen_response,
                connect_side: connect_side.name().to_string(),
                connect_endpoint,
                protocol: spec.protocol,
                status: ForwardPortStatus::Open,
                last_error: None,
            },
            listener_stream_id,
            listen_tunnel,
            cancel,
        },
    })
}

async fn open_udp_forward(
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    listen_endpoint: String,
    connect_endpoint: String,
    spec: ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let listen_tunnel = Arc::new(
        listen_side
            .port_tunnel()
            .await
            .with_context(|| format!("opening port tunnel to `{}`", listen_side.name()))?,
    );
    let connect_tunnel = Arc::new(
        connect_side
            .port_tunnel()
            .await
            .with_context(|| format!("opening port tunnel to `{}`", connect_side.name()))?,
    );
    let listener_stream_id = 1;
    listen_tunnel
        .send(Frame {
            frame_type: FrameType::UdpBind,
            flags: 0,
            stream_id: listener_stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: listen_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| {
            format!(
                "opening udp listener on `{}` at `{listen_endpoint}`",
                listen_side.name()
            )
        })?;
    let listen_response = loop {
        let frame = listen_tunnel.recv().await.with_context(|| {
            format!(
                "waiting for udp listener on `{}` at `{listen_endpoint}`",
                listen_side.name()
            )
        })?;
        match frame.frame_type {
            FrameType::UdpBindOk if frame.stream_id == listener_stream_id => {
                break decode_tunnel_meta::<EndpointMeta>(&frame)?.endpoint;
            }
            FrameType::Error if frame.stream_id == listener_stream_id => {
                return Err(tunnel_error(&frame)).with_context(|| {
                    format!(
                        "opening udp listener on `{}` at `{listen_endpoint}`",
                        listen_side.name()
                    )
                });
            }
            _ => {}
        }
    };

    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: PublicForwardPortProtocol::Udp,
        connect_endpoint: connect_endpoint.clone(),
        listen_tunnel: listen_tunnel.clone(),
        connect_tunnel,
        cancel: cancel.clone(),
    };
    spawn_forward(runtime, store);

    Ok(OpenedForward {
        record: PortForwardRecord {
            entry: ForwardPortEntry {
                forward_id,
                listen_side: listen_side.name().to_string(),
                listen_endpoint: listen_response,
                connect_side: connect_side.name().to_string(),
                connect_endpoint,
                protocol: spec.protocol,
                status: ForwardPortStatus::Open,
                last_error: None,
            },
            listener_stream_id,
            listen_tunnel,
            cancel,
        },
    })
}

pub async fn close_record(record: PortForwardRecord) -> ForwardPortEntry {
    record.cancel.cancel();
    let _ = record
        .listen_tunnel
        .close_stream(record.listener_stream_id)
        .await;
    let mut entry = record.entry;
    entry.status = ForwardPortStatus::Closed;
    entry.last_error = None;
    entry
}

pub async fn close_all(store: &PortForwardStore) {
    for record in store.drain().await {
        let _ = close_record(record).await;
    }
}

fn spawn_forward(runtime: ForwardRuntime, store: PortForwardStore) {
    tokio::spawn(async move {
        let result = match runtime.protocol {
            PublicForwardPortProtocol::Tcp => run_tcp_forward(runtime.clone()).await,
            PublicForwardPortProtocol::Udp => run_udp_forward(runtime.clone()).await,
        };
        if let Err(err) = result {
            runtime.cancel.cancel();
            store
                .mark_failed(&runtime.forward_id, err.to_string())
                .await;
            tracing::warn!(
                forward_id = %runtime.forward_id,
                listen_side = %runtime.listen_side.name(),
                connect_side = %runtime.connect_side.name(),
                error = %err,
                "port forward task stopped"
            );
        }
    });
}

async fn run_tcp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let listen_tunnel = runtime.listen_tunnel.clone();
    let connect_tunnel = runtime.connect_tunnel.clone();
    let paired = Arc::new(Mutex::new(HashMap::<u32, u32>::new()));
    let connect_streams = Arc::new(Mutex::new(HashMap::<u32, u32>::new()));
    let connect_reader = relay_connect_tunnel_to_listen(
        listen_tunnel.clone(),
        connect_tunnel.clone(),
        paired.clone(),
        connect_streams.clone(),
        runtime.cancel.clone(),
    );
    tokio::pin!(connect_reader);

    let mut next_connect_stream_id = 1u32;
    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(()),
            result = &mut connect_reader => return result,
            frame = listen_tunnel.recv() => {
                let frame = frame.context("reading tcp listen tunnel")?;
                match frame.frame_type {
                    FrameType::TcpAccept => {
                        let accept: TcpAcceptMeta = decode_tunnel_meta(&frame)?;
                        let connect_stream_id = next_connect_stream_id;
                        next_connect_stream_id = next_connect_stream_id.checked_add(2).unwrap_or(1);
                        connect_tunnel.send(Frame {
                            frame_type: FrameType::TcpConnect,
                            flags: 0,
                            stream_id: connect_stream_id,
                            meta: encode_tunnel_meta(&EndpointMeta {
                                endpoint: runtime.connect_endpoint.clone(),
                            })?,
                            data: Vec::new(),
                        }).await.context("connecting tcp forward destination")?;
                        paired.lock().await.insert(frame.stream_id, connect_stream_id);
                        connect_streams.lock().await.insert(connect_stream_id, frame.stream_id);
                        tracing::debug!(
                            forward_id = %runtime.forward_id,
                            listener_stream_id = accept.listener_stream_id,
                            accepted_stream_id = frame.stream_id,
                            connect_stream_id,
                            "paired tcp tunnel streams"
                        );
                    }
                    FrameType::TcpData => {
                        if let Some(connect_stream_id) = paired.lock().await.get(&frame.stream_id).copied() {
                            connect_tunnel.send(Frame {
                                stream_id: connect_stream_id,
                                ..frame
                            }).await.context("relaying tcp data to connect tunnel")?;
                        }
                    }
                    FrameType::TcpEof => {
                        if let Some(connect_stream_id) = paired.lock().await.get(&frame.stream_id).copied() {
                            let _ = connect_tunnel.send(Frame {
                                frame_type: frame.frame_type,
                                flags: 0,
                                stream_id: connect_stream_id,
                                meta: Vec::new(),
                                data: Vec::new(),
                            }).await;
                        }
                    }
                    FrameType::Close => {
                        if let Some(connect_stream_id) = paired.lock().await.remove(&frame.stream_id) {
                            connect_streams.lock().await.remove(&connect_stream_id);
                            let _ = connect_tunnel.close_stream(connect_stream_id).await;
                        }
                    }
                    FrameType::Error => return Err(tunnel_error(&frame)).context("listen-side tcp tunnel error"),
                    _ => {}
                }
            }
        }
    }
}

async fn relay_connect_tunnel_to_listen(
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
    listen_to_connect: Arc<Mutex<HashMap<u32, u32>>>,
    connect_to_listen: Arc<Mutex<HashMap<u32, u32>>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            frame = connect_tunnel.recv() => {
                let frame = frame.context("reading tcp connect tunnel")?;
                match frame.frame_type {
                    FrameType::TcpConnectOk => {}
                    FrameType::TcpData => {
                        if let Some(listen_stream_id) = connect_to_listen.lock().await.get(&frame.stream_id).copied() {
                            listen_tunnel.send(Frame {
                                stream_id: listen_stream_id,
                                ..frame
                            }).await.context("relaying tcp data to listen tunnel")?;
                        }
                    }
                    FrameType::TcpEof => {
                        if let Some(listen_stream_id) = connect_to_listen.lock().await.get(&frame.stream_id).copied() {
                            let _ = listen_tunnel.send(Frame {
                                frame_type: frame.frame_type,
                                flags: 0,
                                stream_id: listen_stream_id,
                                meta: Vec::new(),
                                data: Vec::new(),
                            }).await;
                        }
                    }
                    FrameType::Close => {
                        if let Some(listen_stream_id) = connect_to_listen.lock().await.remove(&frame.stream_id) {
                            listen_to_connect.lock().await.remove(&listen_stream_id);
                            let _ = listen_tunnel.close_stream(listen_stream_id).await;
                        }
                    }
                    FrameType::Error => {
                        return Err(tunnel_error(&frame))
                            .context("connecting tcp forward destination");
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn run_udp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let listen_tunnel = runtime.listen_tunnel.clone();
    let connect_tunnel = runtime.connect_tunnel.clone();
    let connector_bind_endpoint = udp_connector_endpoint(&runtime.connect_endpoint)?.to_string();
    let connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let peer_by_connector: Arc<Mutex<HashMap<u32, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let connect_reader = relay_udp_connect_tunnel_to_listen(
        listen_tunnel.clone(),
        connect_tunnel.clone(),
        peer_by_connector.clone(),
        connector_by_peer.clone(),
        runtime.cancel.clone(),
    );
    tokio::pin!(connect_reader);

    let mut next_connector_stream_id = 3;
    let mut sweep = tokio::time::interval(UDP_CONNECTOR_IDLE_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(()),
            result = &mut connect_reader => return result,
            _ = sweep.tick() => {
                sweep_idle_udp_connectors(
                    &connect_tunnel,
                    &connector_by_peer,
                    &peer_by_connector,
                ).await;
            }
            frame = listen_tunnel.recv() => {
                let frame = frame.context("reading udp listen tunnel")?;
                match frame.frame_type {
                    FrameType::UdpDatagram => {
                        let datagram: UdpDatagramMeta = decode_tunnel_meta(&frame)?;
                        let connector_stream_id = udp_connector_stream_id(
                            &connect_tunnel,
                            &connector_by_peer,
                            &peer_by_connector,
                            &mut next_connector_stream_id,
                            &connector_bind_endpoint,
                            datagram.peer.clone(),
                        ).await?;
                        connect_tunnel.send(Frame {
                            frame_type: FrameType::UdpDatagram,
                            flags: 0,
                            stream_id: connector_stream_id,
                            meta: encode_tunnel_meta(&UdpDatagramMeta {
                                peer: runtime.connect_endpoint.clone(),
                            })?,
                            data: frame.data,
                        }).await.context("relaying udp datagram to connect tunnel")?;
                    }
                    FrameType::Close => return Ok(()),
                    FrameType::Error => return Err(tunnel_error(&frame)).context("listen-side udp tunnel error"),
                    _ => {}
                }
            }
        }
    }
}

async fn relay_udp_connect_tunnel_to_listen(
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
    peer_by_connector: Arc<Mutex<HashMap<u32, String>>>,
    connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            frame = connect_tunnel.recv() => {
                let frame = frame.context("reading udp connect tunnel")?;
                match frame.frame_type {
                    FrameType::UdpBindOk => {}
                    FrameType::UdpDatagram => {
                        let Some(peer) = peer_by_connector.lock().await.get(&frame.stream_id).cloned() else {
                            continue;
                        };
                        if let Some(connector) = connector_by_peer.lock().await.get_mut(&peer) {
                            connector.last_used = Instant::now();
                        }
                        listen_tunnel.send(Frame {
                            frame_type: FrameType::UdpDatagram,
                            flags: 0,
                            stream_id: 1,
                            meta: encode_tunnel_meta(&UdpDatagramMeta { peer })?,
                            data: frame.data,
                        }).await.context("relaying udp datagram to listen tunnel")?;
                    }
                    FrameType::Close => {
                        if let Some(peer) = peer_by_connector.lock().await.remove(&frame.stream_id) {
                            connector_by_peer.lock().await.remove(&peer);
                        }
                    }
                    FrameType::Error => {
                        return Err(tunnel_error(&frame)).context("connect-side udp tunnel error");
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn udp_connector_stream_id(
    connect_tunnel: &Arc<PortTunnel>,
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
    next_connector_stream_id: &mut u32,
    connector_bind_endpoint: &str,
    peer: String,
) -> anyhow::Result<u32> {
    if let Some(connector) = connector_by_peer.lock().await.get_mut(&peer) {
        connector.last_used = Instant::now();
        return Ok(connector.stream_id);
    }

    let stream_id = *next_connector_stream_id;
    *next_connector_stream_id = next_connector_stream_id.checked_add(2).unwrap_or(1);
    connect_tunnel
        .send(Frame {
            frame_type: FrameType::UdpBind,
            flags: 0,
            stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: connector_bind_endpoint.to_string(),
            })?,
            data: Vec::new(),
        })
        .await
        .context("opening udp connector stream")?;
    connector_by_peer.lock().await.insert(
        peer.clone(),
        UdpPeerConnector {
            stream_id,
            last_used: Instant::now(),
        },
    );
    peer_by_connector.lock().await.insert(stream_id, peer);
    Ok(stream_id)
}

async fn sweep_idle_udp_connectors(
    connect_tunnel: &Arc<PortTunnel>,
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
) {
    let now = Instant::now();
    let mut expired = Vec::new();
    {
        let connectors = connector_by_peer.lock().await;
        for (peer, connector) in connectors.iter() {
            if now.duration_since(connector.last_used) >= UDP_CONNECTOR_IDLE_TIMEOUT {
                expired.push((peer.clone(), connector.stream_id));
            }
        }
    }
    if expired.is_empty() {
        return;
    }
    let mut connectors = connector_by_peer.lock().await;
    let mut peers = peer_by_connector.lock().await;
    for (peer, stream_id) in expired {
        connectors.remove(&peer);
        peers.remove(&stream_id);
        let _ = connect_tunnel.close_stream(stream_id).await;
    }
}

fn encode_tunnel_meta<T: Serialize>(meta: &T) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec(meta).map_err(anyhow::Error::from)
}

fn decode_tunnel_meta<T: for<'de> Deserialize<'de>>(frame: &Frame) -> anyhow::Result<T> {
    serde_json::from_slice(&frame.meta).map_err(anyhow::Error::from)
}

fn tunnel_error(frame: &Frame) -> anyhow::Error {
    let fallback = || anyhow::anyhow!("port tunnel returned error on stream {}", frame.stream_id);
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&frame.meta) else {
        return fallback();
    };
    let message = value
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or("port tunnel error");
    let code = value.get("code").and_then(|code| code.as_str());
    match code {
        Some(code) => anyhow::anyhow!("{code}: {message}"),
        None => anyhow::anyhow!("{message}"),
    }
}

#[cfg(test)]
mod port_tunnel_tests {
    use remote_exec_proto::port_tunnel::{Frame, FrameType};

    use super::*;

    #[tokio::test]
    async fn local_port_tunnel_binds_tcp_listener() {
        let tunnel = SideHandle::local().port_tunnel().await.unwrap();
        tunnel
            .send(Frame {
                frame_type: FrameType::TcpListen,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "endpoint": "127.0.0.1:0"
                }))
                .unwrap(),
                data: Vec::new(),
            })
            .await
            .unwrap();

        let frame = tunnel.recv().await.unwrap();

        assert_eq!(frame.frame_type, FrameType::TcpListenOk);
    }
}
