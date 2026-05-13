use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use remote_exec_proto::port_tunnel::{Frame, FrameType, TunnelCloseMeta, TunnelRole};
use remote_exec_proto::public::ForwardPortSideRole;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::open::{open_data_tunnel, open_listen_session};
use super::{ForwardRuntime, LISTEN_SESSION_GENERATION, ListenSessionControl};
use crate::port_forward::events::ForwardLoopControl;
use crate::port_forward::tunnel::{
    PortTunnel, decode_tunnel_meta, encode_tunnel_meta, is_retryable_transport_error, tunnel_error,
};
use crate::port_forward::{
    CONNECT_RECONNECT_TOTAL_TIMEOUT, FORWARD_TASK_STOP_TIMEOUT, LISTEN_CLOSE_ACK_TIMEOUT,
    LISTEN_RECONNECT_INITIAL_BACKOFF, LISTEN_RECONNECT_MAX_BACKOFF, LISTEN_RECONNECT_SAFETY_MARGIN,
    PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT,
};

#[derive(Clone, Copy)]
struct PortForwardReconnectPolicy {
    initial_backoff: Duration,
    max_backoff: Duration,
    attempt_timeout: Duration,
    total_timeout: Duration,
    max_attempts: Option<u32>,
}

impl PortForwardReconnectPolicy {
    fn listen(resume_timeout: Duration) -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT,
            total_timeout: effective_resume_timeout(resume_timeout),
            max_attempts: None,
        }
    }

    fn connect() -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: PORT_FORWARD_RECONNECT_ATTEMPT_TIMEOUT,
            total_timeout: CONNECT_RECONNECT_TOTAL_TIMEOUT,
            max_attempts: None,
        }
    }
}

struct RecoveredForwardTunnels {
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
}

pub(in crate::port_forward) async fn wait_for_forward_task_stop(
    task: JoinHandle<()>,
) -> anyhow::Result<()> {
    tokio::time::timeout(FORWARD_TASK_STOP_TIMEOUT, task)
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for port forward task to stop"))?
        .map_err(|err| anyhow::anyhow!("waiting for port forward task to stop: {err}"))
}

async fn resume_listen_session_inner(
    control: &ListenSessionControl,
) -> anyhow::Result<Arc<PortTunnel>> {
    let opened = open_listen_session(
        &control.side,
        &control.forward_id,
        control.protocol,
        LISTEN_SESSION_GENERATION,
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
    let tunnel = resume_listen_session_inner(control).await?;
    let mut state = control.state.lock().await;
    state.current_tunnel = Some(tunnel.clone());
    Ok(tunnel)
}

async fn reconnect_listen_tunnel(
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

async fn reconnect_connect_tunnel(
    runtime: &ForwardRuntime,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    recover_connect_side_tunnel(runtime, "connect-side transport loss").await
}

pub(in crate::port_forward) async fn handle_forward_loop_control<H, Fut>(
    runtime: &ForwardRuntime,
    control: ForwardLoopControl,
    listen_tunnel: &mut Arc<PortTunnel>,
    connect_tunnel: &mut Arc<PortTunnel>,
    before_connect_recover: H,
) -> anyhow::Result<bool>
where
    H: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    match control {
        ForwardLoopControl::Cancelled => Ok(false),
        ForwardLoopControl::RecoverTunnel(TunnelRole::Listen) => {
            let Some(recovered) = recover_listen_side_tunnels(runtime).await? else {
                return Ok(false);
            };
            *listen_tunnel = recovered.listen_tunnel;
            *connect_tunnel = recovered.connect_tunnel;
            Ok(true)
        }
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
            before_connect_recover().await;
            let Some(reconnected_tunnel) = reconnect_connect_tunnel(runtime).await? else {
                return Ok(false);
            };
            *connect_tunnel = reconnected_tunnel;
            Ok(true)
        }
    }
}

async fn recover_listen_side_tunnels(
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
            runtime.forward_id(),
            reason.to_string(),
            runtime.limits.max_reconnecting_forwards,
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
                runtime.connect_side(),
                runtime.forward_id(),
                runtime.protocol(),
                LISTEN_SESSION_GENERATION,
                runtime.limits.max_tunnel_queued_bytes,
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

pub(in crate::port_forward) async fn close_listen_session(
    control: Arc<ListenSessionControl>,
) -> anyhow::Result<()> {
    let current_tunnel = {
        let state = control.state.lock().await;
        state.current_tunnel.clone()
    };
    if let Some(tunnel) = current_tunnel {
        match close_listener_on_tunnel(&tunnel, control.listener_stream_id).await {
            Ok(()) => {
                return close_tunnel_generation(
                    &tunnel,
                    &control.forward_id,
                    LISTEN_SESSION_GENERATION,
                    "operator_close",
                )
                .await;
            }
            Err(err) if is_retryable_transport_error(&err) => {}
            Err(err) => return Err(err),
        }
    }
    let tunnel = resume_listen_session_inner(&control).await?;
    {
        let mut state = control.state.lock().await;
        state.current_tunnel = Some(tunnel.clone());
    }
    close_listener_on_tunnel(&tunnel, control.listener_stream_id).await?;
    close_tunnel_generation(
        &tunnel,
        &control.forward_id,
        LISTEN_SESSION_GENERATION,
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
