use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{Frame, FrameType};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortProtocol as PublicForwardPortProtocol, ForwardPortSpec,
    ForwardPortStatus,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::side::SideHandle;
use super::store::PortForwardRecord;
use super::tcp_bridge::run_tcp_forward;
use super::tunnel::{
    EndpointMeta, PortTunnel, SessionReadyMeta, SessionResumeMeta, decode_tunnel_meta,
    encode_tunnel_meta, is_retryable_transport_error, tunnel_error,
};
use super::udp_bridge::run_udp_forward;
use super::{
    FORWARD_TASK_STOP_TIMEOUT, LISTEN_CLOSE_ACK_TIMEOUT, LISTEN_RECONNECT_INITIAL_BACKOFF,
    LISTEN_RECONNECT_MAX_BACKOFF, LISTEN_RECONNECT_SAFETY_MARGIN,
};

#[derive(Clone)]
pub(super) struct ForwardRuntime {
    pub(super) forward_id: String,
    pub(super) listen_side: SideHandle,
    pub(super) connect_side: SideHandle,
    pub(super) protocol: PublicForwardPortProtocol,
    pub(super) connect_endpoint: String,
    pub(super) listen_session: Arc<ListenSessionControl>,
    pub(super) initial_connect_tunnel: Arc<PortTunnel>,
    pub(super) cancel: CancellationToken,
}

pub(super) struct ListenSessionControl {
    pub(super) side: SideHandle,
    pub(super) session_id: String,
    pub(super) listener_stream_id: u32,
    pub(super) resume_timeout: Duration,
    pub(super) current_tunnel: Mutex<Option<Arc<PortTunnel>>>,
    pub(super) op_lock: Mutex<()>,
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
                listen_endpoint,
                connect_endpoint,
                spec.clone(),
            )
            .await
        }
        PublicForwardPortProtocol::Udp => {
            open_udp_forward(
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
    let task_done = Arc::new(Mutex::new(None));
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
    Ok(OpenedForward {
        task_done: task_done.clone(),
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
            task_done,
        },
        runtime,
    })
}

async fn open_udp_forward(
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
    let task_done = Arc::new(Mutex::new(None));
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
    Ok(OpenedForward {
        task_done: task_done.clone(),
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

pub(super) async fn open_connect_tunnel(side: &SideHandle) -> anyhow::Result<Arc<PortTunnel>> {
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

pub(super) async fn reconnect_listen_tunnel(
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
            Err(err) if is_retryable_transport_error(&err) => {
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

pub(super) async fn close_listen_session(control: Arc<ListenSessionControl>) -> anyhow::Result<()> {
    let _guard = control.op_lock.lock().await;
    if let Some(tunnel) = control.current_tunnel().await {
        match close_listener_on_tunnel(&tunnel, control.listener_stream_id).await {
            Ok(()) => return Ok(()),
            Err(err) if is_retryable_transport_error(&err) => {}
            Err(err) => return Err(err),
        }
    }
    let tunnel = resume_listen_session_inner(&control).await?;
    *control.current_tunnel.lock().await = Some(tunnel.clone());
    close_listener_on_tunnel(&tunnel, control.listener_stream_id).await
}

async fn close_listener_on_tunnel(tunnel: &Arc<PortTunnel>, stream_id: u32) -> anyhow::Result<()> {
    tunnel.close_stream(stream_id).await?;
    wait_for_close_ack(tunnel, stream_id).await
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
