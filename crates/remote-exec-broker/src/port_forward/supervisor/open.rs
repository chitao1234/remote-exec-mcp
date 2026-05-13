use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{
    EndpointMeta, Frame, FrameType, TunnelForwardProtocol, TunnelLimitSummary, TunnelOpenMeta,
    TunnelReadyMeta, TunnelRole,
};
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    ForwardPortSpec,
};
use tokio_util::sync::CancellationToken;

use super::{
    ForwardIdentity, ForwardRuntime, LISTEN_SESSION_GENERATION, LISTEN_SESSION_STREAM_ID,
    ListenSessionControl, ListenSessionParams, OpenedForward,
};
use crate::port_forward::limits::effective_forward_limits;
use crate::port_forward::side::SideHandle;
use crate::port_forward::store::{PortForwardRecord, PortForwardStore};
use crate::port_forward::tunnel::{
    PortTunnel, decode_tunnel_meta, encode_tunnel_meta, tunnel_error,
};
use crate::port_forward::{PORT_FORWARD_OPEN_ACK_TIMEOUT, PORT_FORWARD_TUNNEL_READY_TIMEOUT};

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

pub(super) struct OpenListenSession {
    pub(super) tunnel: Arc<PortTunnel>,
    pub(super) session_id: String,
    pub(super) resume_timeout: Duration,
    pub(super) limits: TunnelLimitSummary,
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

struct TunnelOpenContext {
    opening: &'static str,
    waiting: &'static str,
    timeout: &'static str,
    unexpected: &'static str,
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
        LISTEN_SESSION_GENERATION,
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
    open_data_tunnel(
        connect_side,
        forward_id,
        kind.protocol,
        LISTEN_SESSION_GENERATION,
        max_queued_bytes,
    )
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
    let listener_stream_id = LISTEN_SESSION_STREAM_ID;
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
        listener_stream_id,
        resume_timeout,
        max_tunnel_queued_bytes: limits.max_tunnel_queued_bytes as usize,
        tunnel: listen_tunnel,
    }));

    let cancel = CancellationToken::new();
    let identity = ForwardIdentity::new(
        forward_id.clone(),
        listen_side.clone(),
        connect_side.clone(),
        kind.protocol,
        connect_endpoint.clone(),
    );
    let runtime = ForwardRuntime::new(
        identity,
        limits.into(),
        store,
        listen_session.clone(),
        connect_tunnel,
        cancel.clone(),
    );
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

pub(super) async fn open_listen_session(
    side: &SideHandle,
    forward_id: &str,
    protocol: PublicForwardPortProtocol,
    generation: u64,
    resume_session_id: Option<String>,
    max_queued_bytes: usize,
) -> anyhow::Result<OpenListenSession> {
    let (tunnel, ready) = open_tunnel_with_role(
        side,
        forward_id,
        protocol,
        TunnelRole::Listen,
        generation,
        resume_session_id,
        max_queued_bytes,
        TunnelOpenContext {
            opening: "port tunnel session",
            waiting: "port tunnel session",
            timeout: "port tunnel ready",
            unexpected: "port tunnel session",
        },
    )
    .await?;
    let session_id = ready
        .session_id
        .ok_or_else(|| anyhow::anyhow!("listen tunnel ready did not include session_id"))?;
    let resume_timeout_ms = ready
        .resume_timeout_ms
        .ok_or_else(|| anyhow::anyhow!("listen tunnel ready did not include resume_timeout_ms"))?;
    Ok(OpenListenSession {
        tunnel,
        session_id,
        resume_timeout: Duration::from_millis(resume_timeout_ms),
        limits: ready.limits,
    })
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
    let (tunnel, ready) = open_tunnel_with_role(
        side,
        forward_id,
        protocol,
        TunnelRole::Connect,
        generation,
        None,
        max_queued_bytes,
        TunnelOpenContext {
            opening: "data port tunnel",
            waiting: "data port tunnel",
            timeout: "data port tunnel ready",
            unexpected: "data port tunnel",
        },
    )
    .await?;
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

#[allow(clippy::too_many_arguments)]
async fn open_tunnel_with_role(
    side: &SideHandle,
    forward_id: &str,
    protocol: PublicForwardPortProtocol,
    role: TunnelRole,
    generation: u64,
    resume_session_id: Option<String>,
    max_queued_bytes: usize,
    context: TunnelOpenContext,
) -> anyhow::Result<(Arc<PortTunnel>, TunnelReadyMeta)> {
    let tunnel = open_connect_tunnel(side, max_queued_bytes).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&TunnelOpenMeta {
                forward_id: forward_id.to_string(),
                role,
                side: side.name().to_string(),
                generation,
                protocol: tunnel_protocol(protocol),
                resume_session_id,
            })?,
            data: Vec::new(),
        })
        .await
        .with_context(|| format!("opening {} on `{}`", context.opening, side.name()))?;
    let frame = tokio::time::timeout(PORT_FORWARD_TUNNEL_READY_TIMEOUT, tunnel.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for {}", context.timeout))?
        .with_context(|| format!("waiting for {} on `{}`", context.waiting, side.name()))?;
    match frame.frame_type {
        FrameType::TunnelReady if frame.stream_id == 0 => {
            let ready: TunnelReadyMeta = decode_tunnel_meta(&frame)?;
            Ok((tunnel, ready))
        }
        FrameType::Error if frame.stream_id == 0 => Err(tunnel_error(&frame))
            .with_context(|| format!("opening {} on `{}`", context.opening, side.name())),
        _ => Err(anyhow::anyhow!(
            "unexpected {} response `{:?}` on `{}`",
            context.unexpected,
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
