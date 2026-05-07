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
const LISTEN_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(50);
const LISTEN_RECONNECT_MAX_BACKOFF: Duration = Duration::from_millis(500);
const LISTEN_RECONNECT_SAFETY_MARGIN: Duration = Duration::from_millis(250);

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
    listen_session: Arc<ListenSessionControl>,
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
    listen_session: Arc<ListenSessionControl>,
    initial_connect_tunnel: Arc<PortTunnel>,
    cancel: CancellationToken,
}

struct ListenSessionControl {
    side: SideHandle,
    session_id: String,
    listener_stream_id: u32,
    resume_timeout: Duration,
    current_tunnel: Mutex<Option<Arc<PortTunnel>>>,
    op_lock: Mutex<()>,
}

impl ListenSessionControl {
    fn new(
        side: SideHandle,
        session_id: String,
        listener_stream_id: u32,
        resume_timeout: Duration,
        tunnel: Arc<PortTunnel>,
    ) -> Self {
        Self {
            side,
            session_id,
            listener_stream_id,
            resume_timeout,
            current_tunnel: Mutex::new(Some(tunnel)),
            op_lock: Mutex::new(()),
        }
    }

    async fn current_tunnel(&self) -> Option<Arc<PortTunnel>> {
        self.current_tunnel.lock().await.clone()
    }
}

struct OpenListenSession {
    tunnel: Arc<PortTunnel>,
    session_id: String,
    resume_timeout: Duration,
}

#[derive(Debug, Deserialize, Serialize)]
struct EndpointMeta {
    endpoint: String,
}

#[derive(Debug, Deserialize)]
struct SessionReadyMeta {
    session_id: String,
    resume_timeout_ms: u64,
}

#[derive(Debug, Serialize)]
struct SessionResumeMeta {
    session_id: String,
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

struct TcpConnectStream {
    listen_stream_id: u32,
    ready: bool,
    pending_frames: Vec<Frame>,
}

enum ForwardLoopControl {
    Cancelled,
    ReconnectListenTunnel,
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
    let OpenListenSession {
        tunnel: listen_tunnel,
        session_id,
        resume_timeout,
    } = open_listen_session(&listen_side).await?;
    let connect_tunnel = open_connect_tunnel(&connect_side).await?;
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
    let listen_response = wait_for_listener_ready(
        &listen_tunnel,
        listener_stream_id,
        FrameType::TcpListenOk,
        format!(
            "opening tcp listener on `{}` at `{listen_endpoint}`",
            listen_side.name()
        ),
        format!(
            "waiting for tcp listener on `{}` at `{listen_endpoint}`",
            listen_side.name()
        ),
    )
    .await?;
    let listen_session = Arc::new(ListenSessionControl::new(
        listen_side.clone(),
        session_id,
        listener_stream_id,
        resume_timeout,
        listen_tunnel,
    ));

    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: PublicForwardPortProtocol::Tcp,
        connect_endpoint: connect_endpoint.clone(),
        listen_session: listen_session.clone(),
        initial_connect_tunnel: connect_tunnel,
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
            listen_session,
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
    let OpenListenSession {
        tunnel: listen_tunnel,
        session_id,
        resume_timeout,
    } = open_listen_session(&listen_side).await?;
    let connect_tunnel = open_connect_tunnel(&connect_side).await?;
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
    let listen_response = wait_for_listener_ready(
        &listen_tunnel,
        listener_stream_id,
        FrameType::UdpBindOk,
        format!(
            "opening udp listener on `{}` at `{listen_endpoint}`",
            listen_side.name()
        ),
        format!(
            "waiting for udp listener on `{}` at `{listen_endpoint}`",
            listen_side.name()
        ),
    )
    .await?;
    let listen_session = Arc::new(ListenSessionControl::new(
        listen_side.clone(),
        session_id,
        listener_stream_id,
        resume_timeout,
        listen_tunnel,
    ));

    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: PublicForwardPortProtocol::Udp,
        connect_endpoint: connect_endpoint.clone(),
        listen_session: listen_session.clone(),
        initial_connect_tunnel: connect_tunnel,
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
            listen_session,
            cancel,
        },
    })
}

pub async fn close_record(record: PortForwardRecord) -> ForwardPortEntry {
    record.cancel.cancel();
    let _ = close_listen_session(record.listen_session.clone()).await;
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
            let error_text = format!("{err:#}");
            runtime.cancel.cancel();
            store
                .mark_failed(&runtime.forward_id, error_text.clone())
                .await;
            tracing::warn!(
                forward_id = %runtime.forward_id,
                listen_side = %runtime.listen_side.name(),
                connect_side = %runtime.connect_side.name(),
                error = %error_text,
                "port forward task stopped"
            );
        }
    });
}

async fn run_tcp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let mut listen_tunnel = runtime
        .listen_session
        .current_tunnel()
        .await
        .context("missing listen-side port tunnel")?;
    let mut connect_tunnel = runtime.initial_connect_tunnel.clone();

    loop {
        match run_tcp_forward_epoch(&runtime, listen_tunnel.clone(), connect_tunnel.clone()).await?
        {
            ForwardLoopControl::Cancelled => return Ok(()),
            ForwardLoopControl::ReconnectListenTunnel => {
                let Some(resumed_tunnel) =
                    reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone())
                        .await?
                else {
                    return Ok(());
                };
                listen_tunnel = resumed_tunnel;
                connect_tunnel = open_connect_tunnel(&runtime.connect_side)
                    .await
                    .with_context(|| {
                        format!(
                            "reopening port tunnel to `{}` after listen-side reconnect",
                            runtime.connect_side.name()
                        )
                    })?;
            }
        }
    }
}

async fn run_tcp_forward_epoch(
    runtime: &ForwardRuntime,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> anyhow::Result<ForwardLoopControl> {
    let mut listen_to_connect = HashMap::<u32, u32>::new();
    let mut connect_streams = HashMap::<u32, TcpConnectStream>::new();
    let mut next_connect_stream_id = 1u32;

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            frame = listen_tunnel.recv() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => return classify_listen_transport_failure(err, "reading tcp listen tunnel"),
                };
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
                        listen_to_connect.insert(frame.stream_id, connect_stream_id);
                        connect_streams.insert(connect_stream_id, TcpConnectStream {
                            listen_stream_id: frame.stream_id,
                            ready: false,
                            pending_frames: Vec::new(),
                        });
                        tracing::debug!(
                            forward_id = %runtime.forward_id,
                            listener_stream_id = accept.listener_stream_id,
                            accepted_stream_id = frame.stream_id,
                            connect_stream_id,
                            "paired tcp tunnel streams"
                        );
                    }
                    FrameType::TcpData => {
                        if let Some(connect_stream_id) = listen_to_connect.get(&frame.stream_id).copied() {
                            let remapped = Frame {
                                stream_id: connect_stream_id,
                                ..frame
                            };
                            queue_or_send_tcp_connect_frame(
                                &connect_tunnel,
                                &mut connect_streams,
                                connect_stream_id,
                                remapped,
                            ).await?;
                        }
                    }
                    FrameType::TcpEof => {
                        if let Some(connect_stream_id) = listen_to_connect.get(&frame.stream_id).copied() {
                            let eof = Frame {
                                frame_type: frame.frame_type,
                                flags: 0,
                                stream_id: connect_stream_id,
                                meta: Vec::new(),
                                data: Vec::new(),
                            };
                            queue_or_send_tcp_connect_frame(
                                &connect_tunnel,
                                &mut connect_streams,
                                connect_stream_id,
                                eof,
                            ).await?;
                        }
                    }
                    FrameType::Close => {
                        if let Some(connect_stream_id) = listen_to_connect.remove(&frame.stream_id) {
                            if let Some(stream) = connect_streams.get_mut(&connect_stream_id) {
                                if stream.ready {
                                    let _ = connect_tunnel.close_stream(connect_stream_id).await;
                                    connect_streams.remove(&connect_stream_id);
                                } else {
                                    stream.pending_frames.push(Frame {
                                        frame_type: FrameType::Close,
                                        flags: 0,
                                        stream_id: connect_stream_id,
                                        meta: Vec::new(),
                                        data: Vec::new(),
                                    });
                                }
                            }
                        }
                    }
                    FrameType::Error => return Err(tunnel_error(&frame)).context("listen-side tcp tunnel error"),
                    _ => {}
                }
            }
            frame = connect_tunnel.recv() => {
                let frame = frame.context("reading tcp connect tunnel")?;
                match frame.frame_type {
                    FrameType::TcpConnectOk => {
                        let Some(stream) = connect_streams.get_mut(&frame.stream_id) else {
                            continue;
                        };
                        stream.ready = true;
                        let mut pending = Vec::new();
                        std::mem::swap(&mut pending, &mut stream.pending_frames);
                        let should_remove = flush_pending_tcp_connect_frames(
                            &listen_tunnel,
                            &connect_tunnel,
                            &mut listen_to_connect,
                            &mut connect_streams,
                            frame.stream_id,
                            pending,
                        ).await?;
                        if should_remove {
                            connect_streams.remove(&frame.stream_id);
                        }
                    }
                    FrameType::TcpData => {
                        if let Some(listen_stream_id) = connect_streams
                            .get(&frame.stream_id)
                            .map(|stream| stream.listen_stream_id)
                        {
                            if let Err(err) = listen_tunnel.send(Frame {
                                stream_id: listen_stream_id,
                                ..frame
                            }).await {
                                return classify_listen_transport_failure(err, "relaying tcp data to listen tunnel");
                            }
                        }
                    }
                    FrameType::TcpEof => {
                        if let Some(listen_stream_id) = connect_streams
                            .get(&frame.stream_id)
                            .map(|stream| stream.listen_stream_id)
                        {
                            if let Err(err) = listen_tunnel.send(Frame {
                                frame_type: frame.frame_type,
                                flags: 0,
                                stream_id: listen_stream_id,
                                meta: Vec::new(),
                                data: Vec::new(),
                            }).await {
                                return classify_listen_transport_failure(err, "relaying tcp eof to listen tunnel");
                            }
                        }
                    }
                    FrameType::Close => {
                        if let Some(listen_stream_id) = connect_streams
                            .remove(&frame.stream_id)
                            .map(|stream| stream.listen_stream_id)
                        {
                            listen_to_connect.remove(&listen_stream_id);
                            if let Err(err) = listen_tunnel.close_stream(listen_stream_id).await {
                                return classify_listen_transport_failure(err, "closing tcp listen stream");
                            }
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

async fn queue_or_send_tcp_connect_frame(
    connect_tunnel: &Arc<PortTunnel>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    connect_stream_id: u32,
    frame: Frame,
) -> anyhow::Result<()> {
    let Some(stream) = connect_streams.get_mut(&connect_stream_id) else {
        return Ok(());
    };
    if stream.ready {
        connect_tunnel
            .send(frame)
            .await
            .context("relaying tcp data to connect tunnel")?;
    } else {
        stream.pending_frames.push(frame);
    }
    Ok(())
}

async fn flush_pending_tcp_connect_frames(
    _listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    connect_stream_id: u32,
    pending_frames: Vec<Frame>,
) -> anyhow::Result<bool> {
    let mut should_remove = false;
    for frame in pending_frames {
        let is_close = frame.frame_type == FrameType::Close;
        connect_tunnel
            .send(frame)
            .await
            .context("relaying tcp data to connect tunnel")?;
        if is_close {
            if let Some(listen_stream_id) = connect_streams
                .get(&connect_stream_id)
                .map(|stream| stream.listen_stream_id)
            {
                listen_to_connect.remove(&listen_stream_id);
            }
            should_remove = true;
        }
    }
    Ok(should_remove)
}

async fn run_udp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let mut listen_tunnel = runtime
        .listen_session
        .current_tunnel()
        .await
        .context("missing listen-side port tunnel")?;
    let mut connect_tunnel = runtime.initial_connect_tunnel.clone();

    loop {
        match run_udp_forward_epoch(&runtime, listen_tunnel.clone(), connect_tunnel.clone()).await?
        {
            ForwardLoopControl::Cancelled => return Ok(()),
            ForwardLoopControl::ReconnectListenTunnel => {
                let Some(resumed_tunnel) =
                    reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone())
                        .await?
                else {
                    return Ok(());
                };
                listen_tunnel = resumed_tunnel;
                connect_tunnel = open_connect_tunnel(&runtime.connect_side)
                    .await
                    .with_context(|| {
                        format!(
                            "reopening port tunnel to `{}` after listen-side reconnect",
                            runtime.connect_side.name()
                        )
                    })?;
            }
        }
    }
}

async fn run_udp_forward_epoch(
    runtime: &ForwardRuntime,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> anyhow::Result<ForwardLoopControl> {
    let connector_bind_endpoint = udp_connector_endpoint(&runtime.connect_endpoint)?.to_string();
    let connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let peer_by_connector: Arc<Mutex<HashMap<u32, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut next_connector_stream_id = 3;
    let mut sweep = tokio::time::interval(UDP_CONNECTOR_IDLE_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            _ = sweep.tick() => {
                sweep_idle_udp_connectors(
                    &connect_tunnel,
                    &connector_by_peer,
                    &peer_by_connector,
                ).await;
            }
            frame = listen_tunnel.recv() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => return classify_listen_transport_failure(err, "reading udp listen tunnel"),
                };
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
                    FrameType::Close => return Ok(ForwardLoopControl::Cancelled),
                    FrameType::Error => return Err(tunnel_error(&frame)).context("listen-side udp tunnel error"),
                    _ => {}
                }
            }
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
                        if let Err(err) = listen_tunnel.send(Frame {
                            frame_type: FrameType::UdpDatagram,
                            flags: 0,
                            stream_id: 1,
                            meta: encode_tunnel_meta(&UdpDatagramMeta { peer })?,
                            data: frame.data,
                        }).await {
                            return classify_listen_transport_failure(err, "relaying udp datagram to listen tunnel");
                        }
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

async fn open_listen_session(side: &SideHandle) -> anyhow::Result<OpenListenSession> {
    let tunnel = open_connect_tunnel(side).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionOpen,
            flags: 0,
            stream_id: 0,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await
        .with_context(|| format!("opening port tunnel session on `{}`", side.name()))?;
    let frame = tunnel
        .recv()
        .await
        .with_context(|| format!("waiting for port tunnel session on `{}`", side.name()))?;
    match frame.frame_type {
        FrameType::SessionReady if frame.stream_id == 0 => {
            let ready: SessionReadyMeta = decode_tunnel_meta(&frame)?;
            Ok(OpenListenSession {
                tunnel,
                session_id: ready.session_id,
                resume_timeout: Duration::from_millis(ready.resume_timeout_ms),
            })
        }
        FrameType::Error if frame.stream_id == 0 => Err(tunnel_error(&frame))
            .with_context(|| format!("opening port tunnel session on `{}`", side.name())),
        _ => Err(anyhow::anyhow!(
            "unexpected port tunnel session response `{:?}` on `{}`",
            frame.frame_type,
            side.name()
        )),
    }
}

async fn open_connect_tunnel(side: &SideHandle) -> anyhow::Result<Arc<PortTunnel>> {
    Ok(Arc::new(side.port_tunnel().await.with_context(|| {
        format!("opening port tunnel to `{}`", side.name())
    })?))
}

async fn wait_for_listener_ready(
    tunnel: &Arc<PortTunnel>,
    stream_id: u32,
    ok_type: FrameType,
    open_context: String,
    wait_context: String,
) -> anyhow::Result<String> {
    loop {
        let frame = tunnel.recv().await.with_context(|| wait_context.clone())?;
        match frame.frame_type {
            frame_type if frame_type == ok_type && frame.stream_id == stream_id => {
                return Ok(decode_tunnel_meta::<EndpointMeta>(&frame)?.endpoint);
            }
            FrameType::Error if frame.stream_id == stream_id => {
                return Err(tunnel_error(&frame)).with_context(|| open_context.clone());
            }
            _ => {}
        }
    }
}

async fn resume_listen_session_inner(
    control: &ListenSessionControl,
) -> anyhow::Result<Arc<PortTunnel>> {
    let tunnel = open_connect_tunnel(&control.side).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionResume,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&SessionResumeMeta {
                session_id: control.session_id.clone(),
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| format!("resuming port tunnel session on `{}`", control.side.name()))?;
    let frame = tunnel.recv().await.with_context(|| {
        format!(
            "waiting to resume port tunnel session on `{}`",
            control.side.name()
        )
    })?;
    match frame.frame_type {
        FrameType::SessionResumed if frame.stream_id == 0 => Ok(tunnel),
        FrameType::Error if frame.stream_id == 0 => Err(tunnel_error(&frame))
            .with_context(|| format!("resuming port tunnel session on `{}`", control.side.name())),
        _ => Err(anyhow::anyhow!(
            "unexpected port tunnel resume response `{:?}` on `{}`",
            frame.frame_type,
            control.side.name()
        )),
    }
}

async fn try_resume_listen_tunnel(
    control: &Arc<ListenSessionControl>,
) -> anyhow::Result<Arc<PortTunnel>> {
    let _guard = control.op_lock.lock().await;
    let tunnel = resume_listen_session_inner(control).await?;
    *control.current_tunnel.lock().await = Some(tunnel.clone());
    Ok(tunnel)
}

async fn reconnect_listen_tunnel(
    control: Arc<ListenSessionControl>,
    cancel: CancellationToken,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    let reconnect_window = effective_resume_timeout(control.resume_timeout);
    let deadline = Instant::now() + reconnect_window;
    let mut backoff = LISTEN_RECONNECT_INITIAL_BACKOFF;

    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        match try_resume_listen_tunnel(&control).await {
            Ok(tunnel) => return Ok(Some(tunnel)),
            Err(err) if is_retryable_listen_transport_error(&err) => {
                if Instant::now() >= deadline {
                    break;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                let sleep_for = backoff.min(remaining);
                if sleep_for.is_zero() {
                    break;
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(None),
                    _ = tokio::time::sleep(sleep_for) => {}
                }
                backoff = std::cmp::min(backoff + backoff, LISTEN_RECONNECT_MAX_BACKOFF);
            }
            Err(err) => return Err(err),
        }
    }

    Err(anyhow::anyhow!("port tunnel reconnect timed out"))
}

async fn close_listen_session(control: Arc<ListenSessionControl>) -> anyhow::Result<()> {
    let _guard = control.op_lock.lock().await;
    if let Some(tunnel) = control.current_tunnel().await {
        if tunnel
            .close_stream(control.listener_stream_id)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }

    let tunnel = resume_listen_session_inner(&control).await?;
    *control.current_tunnel.lock().await = Some(tunnel.clone());
    tunnel.close_stream(control.listener_stream_id).await
}

fn classify_listen_transport_failure(
    err: anyhow::Error,
    context: &'static str,
) -> anyhow::Result<ForwardLoopControl> {
    let err = err.context(context);
    if is_retryable_listen_transport_error(&err) {
        Ok(ForwardLoopControl::ReconnectListenTunnel)
    } else {
        Err(err)
    }
}

fn effective_resume_timeout(resume_timeout: Duration) -> Duration {
    let adjusted = resume_timeout.saturating_sub(LISTEN_RECONNECT_SAFETY_MARGIN);
    if adjusted.is_zero() {
        resume_timeout
    } else {
        adjusted
    }
}

fn is_retryable_listen_transport_error(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(daemon_error) = cause.downcast_ref::<DaemonClientError>() {
            if matches!(daemon_error, DaemonClientError::Transport(_)) {
                return true;
            }
        }
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>() {
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::TimedOut
            ) {
                return true;
            }
        }
        let message = cause.to_string();
        if matches!(
            message.as_str(),
            "port tunnel closed" | "port tunnel reader is closed" | "port tunnel writer is closed"
        ) {
            return true;
        }
    }
    false
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
