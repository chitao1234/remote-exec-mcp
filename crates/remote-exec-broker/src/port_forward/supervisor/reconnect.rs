use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TUNNEL_CLOSE_REASON_OPERATOR_CLOSE, TunnelCloseMeta, TunnelRole,
};
use remote_exec_proto::public::ForwardPortSideRole;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::super::epoch::ForwardEpoch;
use super::{ForwardRuntime, ListenSessionControl};
use super::tunnel_open::{open_data_tunnel, open_listen_session};
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
    generation: u64,
) -> anyhow::Result<Arc<PortTunnel>> {
    let opened = open_listen_session(
        &control.side,
        &control.forward_id,
        control.protocol,
        generation,
        Some(control.session_id.clone()),
        control.max_tunnel_queued_bytes,
    )
    .await
    .with_context(|| format!("resuming port tunnel session on `{}`", control.side.name()))?;
    Ok(opened.tunnel)
}

async fn try_resume_listen_tunnel(
    control: &Arc<ListenSessionControl>,
    generation: u64,
) -> anyhow::Result<Arc<PortTunnel>> {
    let tunnel = resume_listen_session_inner(control, generation).await?;
    control
        .replace_current_tunnel(generation, tunnel.clone())
        .await;
    Ok(tunnel)
}

async fn reconnect_listen_tunnel(
    control: Arc<ListenSessionControl>,
    cancel: CancellationToken,
    generation: u64,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    retry_reconnect(
        cancel,
        PortForwardReconnectPolicy::listen(control.resume_timeout),
        || try_resume_listen_tunnel(&control, generation),
    )
    .await
}

async fn reconnect_connect_epoch(runtime: &ForwardRuntime) -> anyhow::Result<Option<ForwardEpoch>> {
    mark_connect_reconnecting(runtime, "connect-side transport loss").await?;
    let generation = runtime.listen_session.advance_generation().await;
    runtime
        .store
        .set_forward_generation(runtime.forward_id(), generation)
        .await;
    let Some(listen_tunnel) = reconnect_listen_tunnel(
        runtime.listen_session.clone(),
        runtime.cancel.clone(),
        generation,
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(connect_tunnel) = retry_open_connect_tunnel(runtime, generation).await? else {
        return Ok(None);
    };
    runtime.mark_active(ForwardPortSideRole::Connect).await;
    Ok(Some(ForwardEpoch::new(
        generation,
        listen_tunnel,
        connect_tunnel,
    )))
}

pub(in crate::port_forward) async fn handle_forward_loop_control<H, Fut>(
    runtime: &ForwardRuntime,
    control: ForwardLoopControl,
    epoch: &mut ForwardEpoch,
    before_connect_recover: H,
) -> anyhow::Result<bool>
where
    H: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    match control {
        ForwardLoopControl::Cancelled => Ok(false),
        ForwardLoopControl::RecoverTunnel(TunnelRole::Listen) => {
            let previous_generation = epoch.generation();
            let Some(recovered_epoch) = recover_listen_side_tunnels(runtime).await? else {
                return Ok(false);
            };
            debug_assert!(recovered_epoch.generation() > previous_generation);
            tracing::debug!(
                forward_id = %runtime.forward_id(),
                failed_role = "listen",
                previous_generation,
                recovered_generation = recovered_epoch.generation(),
                "advanced broker port-forward epoch after listen-side recovery"
            );
            *epoch = recovered_epoch;
            Ok(true)
        }
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
            before_connect_recover().await;
            let previous_generation = epoch.generation();
            let Some(recovered_epoch) = reconnect_connect_epoch(runtime).await? else {
                return Ok(false);
            };
            debug_assert!(recovered_epoch.generation() > previous_generation);
            tracing::debug!(
                forward_id = %runtime.forward_id(),
                failed_role = "connect",
                previous_generation,
                recovered_generation = recovered_epoch.generation(),
                "advanced broker port-forward epoch after connect-side recovery"
            );
            *epoch = recovered_epoch;
            Ok(true)
        }
    }
}

async fn recover_listen_side_tunnels(
    runtime: &ForwardRuntime,
) -> anyhow::Result<Option<ForwardEpoch>> {
    runtime
        .mark_reconnecting(ForwardPortSideRole::Listen, "listen-side tunnel lost")
        .await?;
    let generation = runtime.listen_session.advance_generation().await;
    runtime
        .store
        .set_forward_generation(runtime.forward_id(), generation)
        .await;
    let Some(listen_tunnel) = reconnect_listen_tunnel(
        runtime.listen_session.clone(),
        runtime.cancel.clone(),
        generation,
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(connect_tunnel) = recover_connect_side_tunnel_after_listen_recovery(
        runtime,
        "connect-side tunnel reopening after listen-side recovery",
        generation,
    )
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(ForwardEpoch::new(
        generation,
        listen_tunnel,
        connect_tunnel,
    )))
}

async fn recover_connect_side_tunnel_after_listen_recovery(
    runtime: &ForwardRuntime,
    reason: &str,
    generation: u64,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    runtime
        .store
        .mark_connect_reopening_after_listen_recovery(
            runtime.forward_id(),
            reason.to_string(),
            runtime.limits.max_reconnecting_forwards,
        )
        .await?;
    let Some(connect_tunnel) = retry_open_connect_tunnel(runtime, generation).await? else {
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
    generation: u64,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    retry_reconnect(
        runtime.cancel.clone(),
        PortForwardReconnectPolicy::connect(),
        || async {
            open_data_tunnel(
                runtime.connect_side(),
                runtime.forward_id(),
                runtime.protocol(),
                generation,
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
    let snapshot = control.snapshot().await;
    let Some(tunnel) = snapshot.current_tunnel else {
        return Ok(());
    };
    match close_listener_on_tunnel(&tunnel, control.listener_stream_id).await {
        Ok(()) => {
            return close_tunnel_after_listener_ack(
                &tunnel,
                &control.forward_id,
                snapshot.generation,
                TUNNEL_CLOSE_REASON_OPERATOR_CLOSE,
            )
            .await;
        }
        Err(err) if is_retryable_transport_error(&err) => {}
        Err(err) => return Err(err),
    }
    let generation = snapshot.generation;
    let tunnel = resume_listen_session_inner(&control, generation).await?;
    control
        .replace_current_tunnel(generation, tunnel.clone())
        .await;
    close_listener_on_tunnel(&tunnel, control.listener_stream_id).await?;
    close_tunnel_after_listener_ack(
        &tunnel,
        &control.forward_id,
        generation,
        TUNNEL_CLOSE_REASON_OPERATOR_CLOSE,
    )
    .await
}

async fn close_listener_on_tunnel(tunnel: &Arc<PortTunnel>, stream_id: u32) -> anyhow::Result<()> {
    tunnel.close_stream(stream_id).await?;
    wait_for_close_ack(tunnel, stream_id).await
}

async fn close_tunnel_after_listener_ack(
    tunnel: &Arc<PortTunnel>,
    forward_id: &str,
    generation: u64,
    reason: &str,
) -> anyhow::Result<()> {
    match close_tunnel_generation(tunnel, forward_id, generation, reason).await {
        Ok(()) => Ok(()),
        Err(err) if is_retryable_transport_error(&err) => {
            tracing::debug!(
                forward_id,
                generation,
                error = %err,
                "port tunnel closed after listener close acknowledgement"
            );
            Ok(())
        }
        Err(err) => Err(err),
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, write_frame};
    use remote_exec_proto::public::ForwardPortProtocol;

    use super::super::super::session::LISTEN_SESSION_STREAM_ID;
    use super::super::super::side::SideHandle;
    use super::*;

    #[tokio::test]
    async fn close_listen_session_accepts_transport_loss_after_listener_close_ack() {
        let (broker_side, mut daemon_side) = tokio::io::duplex(4096);
        let tunnel = Arc::new(PortTunnel::from_stream(broker_side).unwrap());
        let control = Arc::new(ListenSessionControl::new_for_test(
            SideHandle::local().unwrap(),
            "fwd_test".to_string(),
            "tunnel_session_test".to_string(),
            ForwardPortProtocol::Tcp,
            Duration::from_secs(10),
            PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            Some(tunnel),
        ));

        let daemon = tokio::spawn(async move {
            let close = read_frame(&mut daemon_side).await.unwrap();
            assert_eq!(close.frame_type, FrameType::Close);
            assert_eq!(close.stream_id, LISTEN_SESSION_STREAM_ID);
            write_frame(
                &mut daemon_side,
                &Frame {
                    frame_type: FrameType::Close,
                    flags: 0,
                    stream_id: close.stream_id,
                    meta: Vec::new(),
                    data: Vec::new(),
                },
            )
            .await
            .unwrap();
        });

        let result = close_listen_session(control).await;
        assert!(
            result.is_ok(),
            "listener close was already acknowledged before transport loss: {result:?}"
        );
        daemon.await.unwrap();
    }

    #[tokio::test]
    async fn close_listen_session_without_retained_tunnel_is_a_noop() {
        let control = Arc::new(ListenSessionControl::new_for_test(
            SideHandle::local().unwrap(),
            "fwd_test".to_string(),
            "tunnel_session_test".to_string(),
            ForwardPortProtocol::Tcp,
            Duration::from_secs(10),
            PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            None,
        ));

        let result = close_listen_session(control).await;
        assert!(
            result.is_ok(),
            "missing retained listen tunnel should not trigger reconnect-on-close: {result:?}"
        );
    }
}
