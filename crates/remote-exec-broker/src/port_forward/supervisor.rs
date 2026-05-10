use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TunnelCloseMeta, TunnelForwardProtocol, TunnelLimitSummary, TunnelOpenMeta,
    TunnelReadyMeta, TunnelRole,
};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    ForwardPortSideRole, ForwardPortSpec,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::limits::effective_forward_limits;
use super::side::SideHandle;
use super::store::{PortForwardRecord, PortForwardStore};
use super::tcp_bridge::run_tcp_forward;
use super::tunnel::{
    EndpointMeta, PortTunnel, decode_tunnel_meta, encode_tunnel_meta, is_retryable_transport_error,
    tunnel_error,
};
use super::udp_bridge::run_udp_forward;
use super::{
    CONNECT_RECONNECT_TOTAL_TIMEOUT, FORWARD_TASK_STOP_TIMEOUT, LISTEN_CLOSE_ACK_TIMEOUT,
    LISTEN_RECONNECT_INITIAL_BACKOFF, LISTEN_RECONNECT_MAX_BACKOFF, LISTEN_RECONNECT_SAFETY_MARGIN,
    PORT_FORWARD_OPEN_ACK_TIMEOUT, PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT,
    PORT_FORWARD_TUNNEL_READY_TIMEOUT,
};

#[derive(Clone, Copy)]
pub(super) struct PortForwardReconnectPolicy {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub attempt_timeout: Duration,
    pub total_timeout: Duration,
    pub max_attempts: Option<u32>,
}

impl PortForwardReconnectPolicy {
    pub(super) fn listen(resume_timeout: Duration) -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT,
            total_timeout: effective_resume_timeout(resume_timeout),
            max_attempts: None,
        }
    }

    pub(super) fn connect() -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT,
            total_timeout: CONNECT_RECONNECT_TOTAL_TIMEOUT,
            max_attempts: None,
        }
    }
}

#[derive(Clone)]
pub(super) struct ForwardRuntime {
    pub(super) forward_id: String,
    pub(super) listen_side: SideHandle,
    pub(super) connect_side: SideHandle,
    pub(super) protocol: PublicForwardPortProtocol,
    pub(super) connect_endpoint: String,
    pub(super) max_active_tcp_streams_per_forward: u64,
    pub(super) max_pending_tcp_bytes_per_stream: usize,
    pub(super) max_pending_tcp_bytes_per_forward: usize,
    pub(super) max_udp_peers_per_forward: usize,
    pub(super) max_tunnel_queued_bytes: usize,
    pub(super) max_reconnecting_forwards: usize,
    pub(super) store: PortForwardStore,
    pub(super) listen_session: Arc<ListenSessionControl>,
    pub(super) initial_connect_tunnel: Arc<PortTunnel>,
    pub(super) cancel: CancellationToken,
}

struct ForwardRuntimeParts {
    forward_id: String,
    listen_side: SideHandle,
    connect_side: SideHandle,
    protocol: PublicForwardPortProtocol,
    connect_endpoint: String,
    limits: ForwardPortLimitSummary,
    store: PortForwardStore,
    listen_session: Arc<ListenSessionControl>,
    initial_connect_tunnel: Arc<PortTunnel>,
    cancel: CancellationToken,
}

impl ForwardRuntime {
    fn new(parts: ForwardRuntimeParts) -> Self {
        Self {
            forward_id: parts.forward_id,
            listen_side: parts.listen_side,
            connect_side: parts.connect_side,
            protocol: parts.protocol,
            connect_endpoint: parts.connect_endpoint,
            max_active_tcp_streams_per_forward: parts.limits.max_active_tcp_streams,
            max_pending_tcp_bytes_per_stream: parts.limits.max_pending_tcp_bytes_per_stream
                as usize,
            max_pending_tcp_bytes_per_forward: parts.limits.max_pending_tcp_bytes_per_forward
                as usize,
            max_udp_peers_per_forward: parts.limits.max_udp_peers as usize,
            max_tunnel_queued_bytes: parts.limits.max_tunnel_queued_bytes as usize,
            max_reconnecting_forwards: parts.limits.max_reconnecting_forwards,
            store: parts.store,
            listen_session: parts.listen_session,
            initial_connect_tunnel: parts.initial_connect_tunnel,
            cancel: parts.cancel,
        }
    }

    pub(super) async fn record_dropped_datagram(&self) {
        self.store
            .update_entry(&self.forward_id, |entry| {
                entry.dropped_udp_datagrams += 1;
            })
            .await;
    }

    pub(super) async fn record_dropped_stream(&self) {
        self.store
            .update_entry(&self.forward_id, |entry| {
                entry.dropped_tcp_streams += 1;
            })
            .await;
    }

    pub(super) async fn record_dropped_streams_and_release_active(&self, count: u64) {
        if count == 0 {
            return;
        }
        self.store
            .update_entry(&self.forward_id, |entry| {
                entry.dropped_tcp_streams += count;
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(count);
            })
            .await;
    }

    pub(super) async fn release_active_stream(&self) {
        self.store
            .update_entry(&self.forward_id, |entry| {
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(1);
            })
            .await;
    }

    pub(super) async fn record_dropped_active_stream(&self) {
        self.store
            .update_entry(&self.forward_id, |entry| {
                entry.dropped_tcp_streams += 1;
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(1);
            })
            .await;
    }

    pub(super) async fn mark_reconnecting(
        &self,
        side: ForwardPortSideRole,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.store
            .mark_reconnecting(
                &self.forward_id,
                side,
                reason.to_string(),
                self.max_reconnecting_forwards,
            )
            .await
    }

    pub(super) async fn mark_active(&self, side: ForwardPortSideRole) {
        self.store.mark_ready(&self.forward_id, side).await;
    }
}

#[derive(Clone, Copy)]
struct ForwardOpenKind {
    protocol: PublicForwardPortProtocol,
    listen_frame_type: FrameType,
    listen_ok_frame_type: FrameType,
    noun: &'static str,
}

#[derive(Clone, Copy)]
enum ForwardSide {
    Listen,
    Connect,
}

impl ForwardOpenKind {
    fn for_protocol(protocol: PublicForwardPortProtocol) -> Self {
        match protocol {
            PublicForwardPortProtocol::Tcp => Self {
                protocol,
                listen_frame_type: FrameType::TcpListen,
                listen_ok_frame_type: FrameType::TcpListenOk,
                noun: "tcp listener",
            },
            PublicForwardPortProtocol::Udp => Self {
                protocol,
                listen_frame_type: FrameType::UdpBind,
                listen_ok_frame_type: FrameType::UdpBindOk,
                noun: "udp listener",
            },
        }
    }
}

pub(super) struct ListenSessionControl {
    pub(super) side: SideHandle,
    pub(super) forward_id: String,
    pub(super) session_id: String,
    pub(super) protocol: PublicForwardPortProtocol,
    pub(super) generation: u64,
    pub(super) listener_stream_id: u32,
    pub(super) resume_timeout: Duration,
    pub(super) max_tunnel_queued_bytes: usize,
    state: Mutex<ListenSessionState>,
}

struct ListenSessionState {
    current_tunnel: Option<Arc<PortTunnel>>,
}

struct ListenSessionParams {
    side: SideHandle,
    forward_id: String,
    session_id: String,
    protocol: PublicForwardPortProtocol,
    generation: u64,
    listener_stream_id: u32,
    resume_timeout: Duration,
    max_tunnel_queued_bytes: usize,
    tunnel: Arc<PortTunnel>,
}

pub struct OpenedForward {
    pub record: PortForwardRecord,
    runtime: ForwardRuntime,
}

impl OpenedForward {
    pub fn entry(&self) -> &ForwardPortEntry {
        &self.record.entry
    }

    pub async fn register_and_start(self, store: super::store::PortForwardStore) {
        let runtime = self.runtime;
        let task = spawn_forward(runtime, store.clone());
        self.record.set_task(task).await;
        store.insert(self.record).await;
    }
}

impl ListenSessionControl {
    fn new(params: ListenSessionParams) -> Self {
        Self {
            side: params.side,
            forward_id: params.forward_id,
            session_id: params.session_id,
            protocol: params.protocol,
            generation: params.generation,
            listener_stream_id: params.listener_stream_id,
            resume_timeout: params.resume_timeout,
            max_tunnel_queued_bytes: params.max_tunnel_queued_bytes,
            state: Mutex::new(ListenSessionState {
                current_tunnel: Some(params.tunnel),
            }),
        }
    }

    pub(super) async fn current_tunnel(&self) -> Option<Arc<PortTunnel>> {
        self.with_session_state(|state| state.current_tunnel.clone())
            .await
    }

    async fn with_session_state<T>(
        &self,
        operation: impl FnOnce(&mut ListenSessionState) -> T,
    ) -> T {
        let mut state = self.state.lock().await;
        operation(&mut state)
    }

    #[cfg(test)]
    pub(super) fn new_for_test(
        side: SideHandle,
        forward_id: String,
        session_id: String,
        protocol: PublicForwardPortProtocol,
        resume_timeout: Duration,
        max_tunnel_queued_bytes: usize,
        tunnel: Option<Arc<PortTunnel>>,
    ) -> Self {
        Self {
            side,
            forward_id,
            session_id,
            protocol,
            generation: 1,
            listener_stream_id: 1,
            resume_timeout,
            max_tunnel_queued_bytes,
            state: Mutex::new(ListenSessionState {
                current_tunnel: tunnel,
            }),
        }
    }
}

struct OpenListenSession {
    tunnel: Arc<PortTunnel>,
    session_id: String,
    resume_timeout: Duration,
    limits: TunnelLimitSummary,
}

struct ForwardOpenContext {
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    forward_id: String,
    listen_endpoint: String,
    connect_endpoint: String,
    requested_limits: ForwardPortLimitSummary,
    kind: ForwardOpenKind,
}

struct OpenedTunnels {
    listen: OpenListenSession,
    connect: OpenDataTunnel,
}

pub(super) struct OpenDataTunnel {
    pub(super) tunnel: Arc<PortTunnel>,
    pub(super) limits: TunnelLimitSummary,
}

pub(super) struct RecoveredForwardTunnels {
    pub(super) listen_tunnel: Arc<PortTunnel>,
    pub(super) connect_tunnel: Arc<PortTunnel>,
}

pub async fn open_forward(
    store: PortForwardStore,
    limits: ForwardPortLimitSummary,
    listen_side: SideHandle,
    connect_side: SideHandle,
    spec: &ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let listen_endpoint = normalize_endpoint(&spec.listen_endpoint)?;
    let connect_endpoint = ensure_nonzero_connect_endpoint(&spec.connect_endpoint)?;
    open_protocol_forward(
        listen_side,
        connect_side,
        store,
        listen_endpoint,
        connect_endpoint,
        limits,
        ForwardOpenKind::for_protocol(spec.protocol),
    )
    .await
}

async fn open_protocol_forward(
    listen_side: SideHandle,
    connect_side: SideHandle,
    store: PortForwardStore,
    listen_endpoint: String,
    connect_endpoint: String,
    limits: ForwardPortLimitSummary,
    kind: ForwardOpenKind,
) -> anyhow::Result<OpenedForward> {
    let forward_id = remote_exec_host::ids::new_forward_id();
    let opened_listen = open_listen_session_for_forward(
        &listen_side,
        &forward_id,
        kind,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
    let opened_connect = open_connect_tunnel_for_forward(
        &connect_side,
        &forward_id,
        kind,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
    build_opened_forward(
        ForwardOpenContext {
            store,
            listen_side,
            connect_side,
            forward_id,
            listen_endpoint,
            connect_endpoint,
            requested_limits: limits,
            kind,
        },
        OpenedTunnels {
            listen: opened_listen,
            connect: opened_connect,
        },
    )
    .await
}

async fn open_listen_session_for_forward(
    listen_side: &SideHandle,
    forward_id: &str,
    kind: ForwardOpenKind,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenListenSession> {
    open_listen_session(
        listen_side,
        forward_id,
        kind.protocol,
        1,
        None,
        max_queued_bytes,
    )
    .await
}

async fn open_connect_tunnel_for_forward(
    connect_side: &SideHandle,
    forward_id: &str,
    kind: ForwardOpenKind,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenDataTunnel> {
    open_data_tunnel(connect_side, forward_id, kind.protocol, 1, max_queued_bytes)
        .await
        .with_context(|| {
            open_context(
                kind,
                ForwardSide::Connect,
                connect_side.name(),
                "data tunnel",
            )
        })
}

async fn build_opened_forward(
    context: ForwardOpenContext,
    opened: OpenedTunnels,
) -> anyhow::Result<OpenedForward> {
    let ForwardOpenContext {
        store,
        listen_side,
        connect_side,
        forward_id,
        listen_endpoint,
        connect_endpoint,
        requested_limits,
        kind,
    } = context;
    let OpenListenSession {
        tunnel: listen_tunnel,
        session_id,
        resume_timeout,
        limits: listen_limits,
    } = opened.listen;
    let limits = effective_forward_limits(requested_limits, &listen_limits, &opened.connect.limits);
    let connect_tunnel = opened.connect.tunnel;
    let listener_stream_id = 1;
    let listener_open_context = open_context(
        kind,
        ForwardSide::Listen,
        listen_side.name(),
        &listen_endpoint,
    );
    listen_tunnel
        .send(Frame {
            frame_type: kind.listen_frame_type,
            flags: 0,
            stream_id: listener_stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: listen_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| listener_open_context.clone())?;
    let listen_response = wait_for_listener_ready(
        &listen_tunnel,
        listener_stream_id,
        kind.listen_ok_frame_type,
        listener_open_context,
        open_context(
            kind,
            ForwardSide::Listen,
            listen_side.name(),
            &listen_endpoint,
        ),
    )
    .await?;
    let listen_session = Arc::new(ListenSessionControl::new(ListenSessionParams {
        side: listen_side.clone(),
        forward_id: forward_id.clone(),
        session_id,
        protocol: kind.protocol,
        generation: 1,
        listener_stream_id,
        resume_timeout,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        tunnel: listen_tunnel,
    }));

    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime::new(ForwardRuntimeParts {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: kind.protocol,
        connect_endpoint: connect_endpoint.clone(),
        limits,
        store,
        listen_session: listen_session.clone(),
        initial_connect_tunnel: connect_tunnel,
        cancel: cancel.clone(),
    });
    Ok(OpenedForward {
        record: PortForwardRecord::new(
            ForwardPortEntry::new_open(
                forward_id,
                listen_side.name().to_string(),
                listen_response,
                connect_side.name().to_string(),
                connect_endpoint,
                kind.protocol,
                limits,
            ),
            listen_session,
            cancel,
        ),
        runtime,
    })
}

fn open_context(kind: ForwardOpenKind, side: ForwardSide, target: &str, endpoint: &str) -> String {
    match side {
        ForwardSide::Listen => format!("opening {} on `{target}` at `{endpoint}`", kind.noun),
        ForwardSide::Connect => format!(
            "opening {} data port tunnel on `{target}`",
            forward_protocol_name(kind.protocol)
        ),
    }
}

fn forward_protocol_name(protocol: PublicForwardPortProtocol) -> &'static str {
    match protocol {
        PublicForwardPortProtocol::Tcp => "tcp",
        PublicForwardPortProtocol::Udp => "udp",
    }
}

fn spawn_forward(runtime: ForwardRuntime, store: super::store::PortForwardStore) -> JoinHandle<()> {
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
    })
}

pub(super) async fn wait_for_forward_task_stop(task: JoinHandle<()>) -> anyhow::Result<()> {
    tokio::time::timeout(FORWARD_TASK_STOP_TIMEOUT, task)
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for port forward task to stop"))?
        .map_err(|err| anyhow::anyhow!("waiting for port forward task to stop: {err}"))
}

async fn open_listen_session(
    side: &SideHandle,
    forward_id: &str,
    protocol: PublicForwardPortProtocol,
    generation: u64,
    resume_session_id: Option<String>,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenListenSession> {
    let tunnel = open_connect_tunnel(side, max_queued_bytes).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&TunnelOpenMeta {
                forward_id: forward_id.to_string(),
                role: TunnelRole::Listen,
                side: side.name().to_string(),
                generation,
                protocol: tunnel_protocol(protocol),
                resume_session_id,
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| format!("opening port tunnel session on `{}`", side.name()))?;
    let frame = tokio::time::timeout(PORT_FORWARD_TUNNEL_READY_TIMEOUT, tunnel.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel ready"))?
        .with_context(|| format!("waiting for port tunnel session on `{}`", side.name()))?;
    match frame.frame_type {
        FrameType::TunnelReady if frame.stream_id == 0 => {
            let ready: TunnelReadyMeta = decode_tunnel_meta(&frame)?;
            let session_id = ready
                .session_id
                .ok_or_else(|| anyhow::anyhow!("listen tunnel ready did not include session_id"))?;
            let resume_timeout_ms = ready.resume_timeout_ms.ok_or_else(|| {
                anyhow::anyhow!("listen tunnel ready did not include resume_timeout_ms")
            })?;
            Ok(OpenListenSession {
                tunnel,
                session_id,
                resume_timeout: Duration::from_millis(resume_timeout_ms),
                limits: ready.limits,
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

fn tunnel_protocol(protocol: PublicForwardPortProtocol) -> TunnelForwardProtocol {
    match protocol {
        PublicForwardPortProtocol::Tcp => TunnelForwardProtocol::Tcp,
        PublicForwardPortProtocol::Udp => TunnelForwardProtocol::Udp,
    }
}

pub(super) async fn open_connect_tunnel(
    side: &SideHandle,
    max_queued_bytes: usize,
) -> anyhow::Result<Arc<PortTunnel>> {
    Ok(Arc::new(
        side.port_tunnel(max_queued_bytes)
            .await
            .with_context(|| format!("opening port tunnel to `{}`", side.name()))?,
    ))
}

pub(super) async fn open_data_tunnel(
    side: &SideHandle,
    forward_id: &str,
    protocol: PublicForwardPortProtocol,
    generation: u64,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenDataTunnel> {
    let tunnel = open_connect_tunnel(side, max_queued_bytes).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&TunnelOpenMeta {
                forward_id: forward_id.to_string(),
                role: TunnelRole::Connect,
                side: side.name().to_string(),
                generation,
                protocol: tunnel_protocol(protocol),
                resume_session_id: None,
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| format!("opening data port tunnel on `{}`", side.name()))?;
    let frame = tokio::time::timeout(PORT_FORWARD_TUNNEL_READY_TIMEOUT, tunnel.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for data port tunnel ready"))?
        .with_context(|| format!("waiting for data port tunnel on `{}`", side.name()))?;
    match frame.frame_type {
        FrameType::TunnelReady if frame.stream_id == 0 => {
            let ready: TunnelReadyMeta = decode_tunnel_meta(&frame)?;
            if ready.generation != generation {
                anyhow::bail!(
                    "data port tunnel on `{}` returned generation `{}` instead of `{generation}`",
                    side.name(),
                    ready.generation
                );
            }
            Ok(OpenDataTunnel {
                tunnel,
                limits: ready.limits,
            })
        }
        FrameType::Error if frame.stream_id == 0 => Err(tunnel_error(&frame))
            .with_context(|| format!("opening data port tunnel on `{}`", side.name())),
        _ => Err(anyhow::anyhow!(
            "unexpected data port tunnel response `{:?}` on `{}`",
            frame.frame_type,
            side.name()
        )),
    }
}

async fn wait_for_listener_ready(
    tunnel: &Arc<PortTunnel>,
    stream_id: u32,
    ok_type: FrameType,
    open_context: String,
    wait_context: String,
) -> anyhow::Result<String> {
    loop {
        let frame = tokio::time::timeout(PORT_FORWARD_OPEN_ACK_TIMEOUT, tunnel.recv())
            .await
            .map_err(|_| {
                anyhow::anyhow!("timed out waiting for port forward listener acknowledgement")
            })?
            .with_context(|| wait_context.clone())?;
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
    let opened = open_listen_session(
        &control.side,
        &control.forward_id,
        control.protocol,
        control.generation,
        Some(control.session_id.clone()),
        control.max_tunnel_queued_bytes,
    )
    .await
    .with_context(|| format!("resuming port tunnel session on `{}`", control.side.name()))?;
    Ok(opened.tunnel)
}

async fn try_resume_listen_tunnel(
    control: &Arc<ListenSessionControl>,
) -> anyhow::Result<Arc<PortTunnel>> {
    let mut state = control.state.lock().await;
    let tunnel = resume_listen_session_inner(control).await?;
    state.current_tunnel = Some(tunnel.clone());
    Ok(tunnel)
}

pub(super) async fn reconnect_listen_tunnel(
    control: Arc<ListenSessionControl>,
    cancel: CancellationToken,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    retry_reconnect(
        cancel,
        PortForwardReconnectPolicy::listen(control.resume_timeout),
        || try_resume_listen_tunnel(&control),
    )
    .await
}

pub(super) async fn reconnect_connect_tunnel(
    runtime: &ForwardRuntime,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    recover_connect_side_tunnel(runtime, "connect-side transport loss").await
}

pub(super) async fn recover_listen_side_tunnels(
    runtime: &ForwardRuntime,
) -> anyhow::Result<Option<RecoveredForwardTunnels>> {
    runtime
        .mark_reconnecting(ForwardPortSideRole::Listen, "listen-side tunnel lost")
        .await?;
    let Some(listen_tunnel) =
        reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone()).await?
    else {
        return Ok(None);
    };
    let Some(connect_tunnel) = recover_connect_side_tunnel_after_listen_recovery(
        runtime,
        "connect-side tunnel reopening after listen-side recovery",
    )
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(RecoveredForwardTunnels {
        listen_tunnel,
        connect_tunnel,
    }))
}

async fn recover_connect_side_tunnel_after_listen_recovery(
    runtime: &ForwardRuntime,
    reason: &str,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    runtime
        .store
        .mark_connect_reopening_after_listen_recovery(
            &runtime.forward_id,
            reason.to_string(),
            runtime.max_reconnecting_forwards,
        )
        .await?;
    let Some(connect_tunnel) = retry_open_connect_tunnel(runtime).await? else {
        return Ok(None);
    };
    runtime.mark_active(ForwardPortSideRole::Connect).await;
    Ok(Some(connect_tunnel))
}

async fn recover_connect_side_tunnel(
    runtime: &ForwardRuntime,
    reason: &str,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    mark_connect_reconnecting(runtime, reason).await?;
    let Some(connect_tunnel) = retry_open_connect_tunnel(runtime).await? else {
        return Ok(None);
    };
    runtime.mark_active(ForwardPortSideRole::Connect).await;
    Ok(Some(connect_tunnel))
}

async fn mark_connect_reconnecting(runtime: &ForwardRuntime, reason: &str) -> anyhow::Result<()> {
    runtime
        .mark_reconnecting(ForwardPortSideRole::Connect, reason)
        .await
}

async fn retry_open_connect_tunnel(
    runtime: &ForwardRuntime,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    retry_reconnect(
        runtime.cancel.clone(),
        PortForwardReconnectPolicy::connect(),
        || async {
            open_data_tunnel(
                &runtime.connect_side,
                &runtime.forward_id,
                runtime.protocol,
                1,
                runtime.max_tunnel_queued_bytes,
            )
            .await
            .map(|opened| opened.tunnel)
        },
    )
    .await
}

async fn retry_reconnect<T, Fut>(
    cancel: CancellationToken,
    policy: PortForwardReconnectPolicy,
    mut attempt_fn: impl FnMut() -> Fut,
) -> anyhow::Result<Option<T>>
where
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let deadline = Instant::now() + policy.total_timeout;
    let mut backoff = policy.initial_backoff;
    let mut attempts = 0u32;
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if policy.max_attempts.is_some_and(|max| attempts >= max) {
            return Err(anyhow::anyhow!("port forward reconnect attempts exhausted"));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(anyhow::anyhow!("port tunnel reconnect timed out"));
        }
        attempts += 1;
        let attempt_timeout = policy.attempt_timeout.min(remaining);
        let result = tokio::select! {
            _ = cancel.cancelled() => return Ok(None),
            result = tokio::time::timeout(attempt_timeout, attempt_fn()) => result,
        };
        match result {
            Ok(Ok(value)) => return Ok(Some(value)),
            Ok(Err(err)) if is_retryable_transport_error(&err) => {}
            Ok(Err(err)) => return Err(err),
            Err(_) => {}
        }
        let sleep_for = backoff.min(deadline.saturating_duration_since(Instant::now()));
        if sleep_for.is_zero() {
            continue;
        }
        tokio::select! {
            _ = cancel.cancelled() => return Ok(None),
            _ = tokio::time::sleep(sleep_for) => {}
        }
        backoff = std::cmp::min(backoff + backoff, policy.max_backoff);
    }
}

pub(super) async fn close_listen_session(control: Arc<ListenSessionControl>) -> anyhow::Result<()> {
    let mut state = control.state.lock().await;
    if let Some(tunnel) = state.current_tunnel.clone() {
        match close_listener_on_tunnel(&tunnel, control.listener_stream_id).await {
            Ok(()) => {
                return close_tunnel_generation(
                    &tunnel,
                    &control.forward_id,
                    control.generation,
                    "operator_close",
                )
                .await;
            }
            Err(err) if is_retryable_transport_error(&err) => {}
            Err(err) => return Err(err),
        }
    }
    let tunnel = resume_listen_session_inner(&control).await?;
    state.current_tunnel = Some(tunnel.clone());
    close_listener_on_tunnel(&tunnel, control.listener_stream_id).await?;
    close_tunnel_generation(
        &tunnel,
        &control.forward_id,
        control.generation,
        "operator_close",
    )
    .await
}

async fn close_listener_on_tunnel(tunnel: &Arc<PortTunnel>, stream_id: u32) -> anyhow::Result<()> {
    tunnel.close_stream(stream_id).await?;
    wait_for_close_ack(tunnel, stream_id).await
}

async fn close_tunnel_generation(
    tunnel: &Arc<PortTunnel>,
    forward_id: &str,
    generation: u64,
    reason: &str,
) -> anyhow::Result<()> {
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&TunnelCloseMeta {
                forward_id: forward_id.to_string(),
                generation,
                reason: reason.to_string(),
            })?,
            data: Vec::new(),
        })
        .await?;
    wait_for_tunnel_closed(tunnel, generation).await
}

async fn wait_for_tunnel_closed(tunnel: &Arc<PortTunnel>, generation: u64) -> anyhow::Result<()> {
    tokio::time::timeout(LISTEN_CLOSE_ACK_TIMEOUT, async {
        loop {
            let frame = tunnel.recv().await?;
            match frame.frame_type {
                FrameType::TunnelClosed if frame.stream_id == 0 => {
                    let closed: TunnelCloseMeta = decode_tunnel_meta(&frame)?;
                    if closed.generation == generation {
                        return Ok(());
                    }
                }
                FrameType::Error if frame.stream_id == 0 => return Err(tunnel_error(&frame)),
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel close acknowledgement"))?
}

async fn wait_for_close_ack(tunnel: &Arc<PortTunnel>, stream_id: u32) -> anyhow::Result<()> {
    tokio::time::timeout(LISTEN_CLOSE_ACK_TIMEOUT, async {
        loop {
            let frame = tunnel.recv().await?;
            match frame.frame_type {
                FrameType::Close if frame.stream_id == stream_id => return Ok(()),
                FrameType::Error if frame.stream_id == stream_id => {
                    return Err(tunnel_error(&frame));
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for port forward close acknowledgement"))?
}

fn effective_resume_timeout(resume_timeout: Duration) -> Duration {
    let adjusted = resume_timeout.saturating_sub(LISTEN_RECONNECT_SAFETY_MARGIN);
    if adjusted.is_zero() {
        resume_timeout
    } else {
        adjusted
    }
}
