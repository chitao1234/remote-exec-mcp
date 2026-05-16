use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TunnelForwardProtocol, TunnelLimitSummary, TunnelOpenMeta, TunnelReadyMeta,
    TunnelRole,
};
use remote_exec_proto::public::ForwardPortProtocol as PublicForwardPortProtocol;

use crate::port_forward::side::SideHandle;
use crate::port_forward::tunnel::{
    PortTunnel, decode_tunnel_meta, encode_tunnel_meta, tunnel_error,
};
use crate::port_forward::PORT_FORWARD_TUNNEL_READY_TIMEOUT;

pub(super) struct OpenListenSession {
    pub(super) tunnel: Arc<PortTunnel>,
    pub(super) session_id: String,
    pub(super) resume_timeout: Duration,
    pub(super) limits: TunnelLimitSummary,
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
                protocol: TunnelForwardProtocol::from(protocol),
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
