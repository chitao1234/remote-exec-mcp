use crate::HostRpcError;
use remote_exec_proto::rpc::RpcErrorCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionCloseMode {
    GracefulClose,
    RetryableDetach,
    TerminalFailure,
}

pub(super) fn rpc_error(code: RpcErrorCode, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
    crate::error::bad_request(code, message)
}

pub(super) fn is_recoverable_pressure_error(error: &HostRpcError) -> bool {
    error.code() == Some(RpcErrorCode::PortTunnelLimitExceeded)
}
