use crate::HostRpcError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionCloseMode {
    GracefulClose,
    RetryableDetach,
    TerminalFailure,
}

pub(super) fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    HostRpcError {
        status: 400,
        code,
        message,
    }
}
