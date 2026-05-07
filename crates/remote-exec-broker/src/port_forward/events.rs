use remote_exec_proto::port_tunnel::Frame;

use super::tunnel::is_retryable_transport_error;

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

#[allow(dead_code, reason = "Connect-side recovery is introduced in follow-up tasks")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TunnelRole {
    Listen,
    Connect,
}

pub(super) enum ForwardLoopControl {
    Cancelled,
    RecoverTunnel(TunnelRole),
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
