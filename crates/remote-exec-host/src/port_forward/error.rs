use crate::HostRpcError;
use remote_exec_proto::rpc::RpcErrorCode;

pub(super) use crate::error::logged_bad_request as rpc_error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionCloseMode {
    GracefulClose,
    RetryableDetach,
    TerminalFailure,
}

pub(super) fn is_recoverable_pressure_error(error: &HostRpcError) -> bool {
    error.code() == Some(RpcErrorCode::PortTunnelLimitExceeded)
}
