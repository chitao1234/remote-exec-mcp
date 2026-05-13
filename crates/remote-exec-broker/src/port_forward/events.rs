use std::future::Future;

use anyhow::Context;
use remote_exec_proto::port_tunnel::Frame;

use super::tunnel::{
    classify_recoverable_tunnel_event, format_terminal_tunnel_error, is_retryable_transport_error,
};

pub(super) use remote_exec_proto::port_tunnel::TunnelRole;

pub(super) enum ForwardSideEvent {
    Frame(Frame),
    RetryableTransportLoss,
    TerminalTransportError(anyhow::Error),
    TerminalTunnelError(TunnelErrorMeta),
}

#[derive(Clone, Debug)]
pub(super) struct TunnelErrorMeta {
    pub(super) code: Option<String>,
    pub(super) message: String,
    pub(super) fatal: bool,
    pub(super) stream_id: u32,
}

pub(super) enum ForwardLoopControl {
    Cancelled,
    RecoverTunnel(TunnelRole),
}

pub(super) enum TunnelFrameOutcome {
    Frame(Frame),
    Control(ForwardLoopControl),
}

pub(super) fn classify_transport_failure(
    err: anyhow::Error,
    context: &'static str,
    role: TunnelRole,
) -> anyhow::Result<ForwardLoopControl> {
    let err = err.context(context);
    if is_retryable_transport_error(&err) {
        Ok(ForwardLoopControl::RecoverTunnel(role))
    } else {
        Err(err)
    }
}

pub(super) async fn recoverable_tunnel_frame<F, Fut>(
    result: anyhow::Result<Frame>,
    transport_context: &'static str,
    tunnel_context: &'static str,
    on_retryable_transport_loss: F,
) -> anyhow::Result<TunnelFrameOutcome>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<ForwardLoopControl>>,
{
    match classify_recoverable_tunnel_event(result) {
        ForwardSideEvent::Frame(frame) => Ok(TunnelFrameOutcome::Frame(frame)),
        ForwardSideEvent::RetryableTransportLoss => Ok(TunnelFrameOutcome::Control(
            on_retryable_transport_loss().await?,
        )),
        ForwardSideEvent::TerminalTransportError(err) => Err(err).context(transport_context),
        ForwardSideEvent::TerminalTunnelError(meta) => {
            Err(format_terminal_tunnel_error(&meta)).context(tunnel_context)
        }
    }
}
