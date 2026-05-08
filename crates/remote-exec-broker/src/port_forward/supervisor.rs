use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TunnelCloseMeta, TunnelForwardProtocol, TunnelOpenMeta, TunnelReadyMeta,
    TunnelRole,
};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    ForwardPortSideRole, ForwardPortSpec,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::side::SideHandle;
use super::store::{PortForwardRecord, PortForwardStore};
use super::tcp_bridge::run_tcp_forward;
use super::tunnel::{
    EndpointMeta, PortTunnel, SessionResumeMeta, decode_tunnel_meta, encode_tunnel_meta,
    is_retryable_transport_error, tunnel_error,
};
use super::udp_bridge::run_udp_forward;
use super::{
    FORWARD_TASK_STOP_TIMEOUT, LISTEN_CLOSE_ACK_TIMEOUT, LISTEN_RECONNECT_INITIAL_BACKOFF,
    LISTEN_RECONNECT_MAX_BACKOFF, LISTEN_RECONNECT_SAFETY_MARGIN, PORT_FORWARD_OPEN_ACK_TIMEOUT,
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
            attempt_timeout: Duration::from_secs(2),
            total_timeout: effective_resume_timeout(resume_timeout),
            max_attempts: None,
        }
    }

    pub(super) fn connect() -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(10),
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
    pub(super) store: PortForwardStore,
    pub(super) listen_session: Arc<ListenSessionControl>,
    pub(super) initial_connect_tunnel: Arc<PortTunnel>,
    pub(super) cancel: CancellationToken,
}

pub(super) struct ListenSessionControl {
    pub(super) side: SideHandle,
    pub(super) forward_id: String,
    pub(super) session_id: String,
    pub(super) generation: u64,
    pub(super) listener_stream_id: u32,
    pub(super) resume_timeout: Duration,
    pub(super) max_tunnel_queued_bytes: usize,
    pub(super) current_tunnel: Mutex<Option<Arc<PortTunnel>>>,
    pub(super) op_lock: Mutex<()>,
}

struct ListenSessionParams {
    side: SideHandle,
    forward_id: String,
    session_id: String,
    generation: u64,
    listener_stream_id: u32,
    resume_timeout: Duration,
    max_tunnel_queued_bytes: usize,
    tunnel: Arc<PortTunnel>,
}

pub struct OpenedForward {
    pub record: PortForwardRecord,
    runtime: ForwardRuntime,
    task_done: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl OpenedForward {
    pub fn entry(&self) -> &ForwardPortEntry {
        &self.record.entry
    }

    pub async fn register_and_start(self, store: super::store::PortForwardStore) {
        let runtime = self.runtime;
        let task_done = self.task_done.clone();
        store.insert(self.record).await;
        let task = spawn_forward(runtime, store);
        *task_done.lock().await = Some(task);
    }
}

impl ListenSessionControl {
    fn new(params: ListenSessionParams) -> Self {
        Self {
            side: params.side,
            forward_id: params.forward_id,
            session_id: params.session_id,
            generation: params.generation,
            listener_stream_id: params.listener_stream_id,
            resume_timeout: params.resume_timeout,
            max_tunnel_queued_bytes: params.max_tunnel_queued_bytes,
            current_tunnel: Mutex::new(Some(params.tunnel)),
            op_lock: Mutex::new(()),
        }
    }

    pub(super) async fn current_tunnel(&self) -> Option<Arc<PortTunnel>> {
        self.current_tunnel.lock().await.clone()
    }
}

struct OpenListenSession {
    tunnel: Arc<PortTunnel>,
    session_id: String,
    resume_timeout: Duration,
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
    match spec.protocol {
        PublicForwardPortProtocol::Tcp => {
            open_tcp_forward(
                listen_side,
                connect_side,
                store,
                listen_endpoint,
                connect_endpoint,
                limits,
                spec.clone(),
            )
            .await
        }
        PublicForwardPortProtocol::Udp => {
            open_udp_forward(
                listen_side,
                connect_side,
                store,
                listen_endpoint,
                connect_endpoint,
                limits,
                spec.clone(),
            )
            .await
        }
    }
}

async fn open_tcp_forward(
    listen_side: SideHandle,
    connect_side: SideHandle,
    store: PortForwardStore,
    listen_endpoint: String,
    connect_endpoint: String,
    limits: ForwardPortLimitSummary,
    spec: ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let OpenListenSession {
        tunnel: listen_tunnel,
        session_id,
        resume_timeout,
    } = open_listen_session(
        &listen_side,
        &forward_id,
        PublicForwardPortProtocol::Tcp,
        1,
        None,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
    let connect_tunnel = open_data_tunnel(
        &connect_side,
        &forward_id,
        PublicForwardPortProtocol::Tcp,
        1,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
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
    let listen_session = Arc::new(ListenSessionControl::new(ListenSessionParams {
        side: listen_side.clone(),
        forward_id: forward_id.clone(),
        session_id,
        generation: 1,
        listener_stream_id,
        resume_timeout,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        tunnel: listen_tunnel,
    }));

    let cancel = CancellationToken::new();
    let task_done = Arc::new(Mutex::new(None));
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: PublicForwardPortProtocol::Tcp,
        connect_endpoint: connect_endpoint.clone(),
        max_active_tcp_streams_per_forward: limits.max_active_tcp_streams,
        max_pending_tcp_bytes_per_stream: limits.max_pending_tcp_bytes_per_stream as usize,
        max_pending_tcp_bytes_per_forward: limits.max_pending_tcp_bytes_per_forward as usize,
        max_udp_peers_per_forward: limits.max_udp_peers as usize,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        store,
        listen_session: listen_session.clone(),
        initial_connect_tunnel: connect_tunnel,
        cancel: cancel.clone(),
    };
    Ok(OpenedForward {
        task_done: task_done.clone(),
        record: PortForwardRecord {
            entry: ForwardPortEntry::new_open(
                forward_id,
                listen_side.name().to_string(),
                listen_response,
                connect_side.name().to_string(),
                connect_endpoint,
                spec.protocol,
                limits,
            ),
            listen_session,
            cancel,
            task_done,
        },
        runtime,
    })
}

async fn open_udp_forward(
    listen_side: SideHandle,
    connect_side: SideHandle,
    store: PortForwardStore,
    listen_endpoint: String,
    connect_endpoint: String,
    limits: ForwardPortLimitSummary,
    spec: ForwardPortSpec,
) -> anyhow::Result<OpenedForward> {
    let forward_id = format!("fwd_{}", uuid::Uuid::new_v4().simple());
    let OpenListenSession {
        tunnel: listen_tunnel,
        session_id,
        resume_timeout,
    } = open_listen_session(
        &listen_side,
        &forward_id,
        PublicForwardPortProtocol::Udp,
        1,
        None,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
    let connect_tunnel = open_data_tunnel(
        &connect_side,
        &forward_id,
        PublicForwardPortProtocol::Udp,
        1,
        limits.max_tunnel_queued_bytes as usize,
    )
    .await?;
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
    let listen_session = Arc::new(ListenSessionControl::new(ListenSessionParams {
        side: listen_side.clone(),
        forward_id: forward_id.clone(),
        session_id,
        generation: 1,
        listener_stream_id,
        resume_timeout,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        tunnel: listen_tunnel,
    }));

    let cancel = CancellationToken::new();
    let task_done = Arc::new(Mutex::new(None));
    let runtime = ForwardRuntime {
        forward_id: forward_id.clone(),
        listen_side: listen_side.clone(),
        connect_side: connect_side.clone(),
        protocol: PublicForwardPortProtocol::Udp,
        connect_endpoint: connect_endpoint.clone(),
        max_active_tcp_streams_per_forward: limits.max_active_tcp_streams,
        max_pending_tcp_bytes_per_stream: limits.max_pending_tcp_bytes_per_stream as usize,
        max_pending_tcp_bytes_per_forward: limits.max_pending_tcp_bytes_per_forward as usize,
        max_udp_peers_per_forward: limits.max_udp_peers as usize,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        store,
        listen_session: listen_session.clone(),
        initial_connect_tunnel: connect_tunnel,
        cancel: cancel.clone(),
    };
    Ok(OpenedForward {
        task_done: task_done.clone(),
        record: PortForwardRecord {
            entry: ForwardPortEntry::new_open(
                forward_id,
                listen_side.name().to_string(),
                listen_response,
                connect_side.name().to_string(),
                connect_endpoint,
                spec.protocol,
                limits,
            ),
            listen_session,
            cancel,
            task_done,
        },
        runtime,
    })
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
) -> anyhow::Result<Arc<PortTunnel>> {
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
            Ok(tunnel)
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
    let tunnel = open_connect_tunnel(&control.side, control.max_tunnel_queued_bytes).await?;
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
    runtime
        .store
        .mark_reconnecting(
            &runtime.forward_id,
            ForwardPortSideRole::Connect,
            "connect-side transport loss".to_string(),
        )
        .await;
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
    let _guard = control.op_lock.lock().await;
    if let Some(tunnel) = control.current_tunnel().await {
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
    *control.current_tunnel.lock().await = Some(tunnel.clone());
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
