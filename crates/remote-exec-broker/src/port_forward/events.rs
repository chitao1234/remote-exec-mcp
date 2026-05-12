use remote_exec_proto::port_tunnel::Frame;

use super::tunnel::is_retryable_transport_error;

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
