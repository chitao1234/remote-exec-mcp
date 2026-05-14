use crate::HostRpcError;
use remote_exec_proto::rpc::RpcErrorCode;

pub(super) fn request_error(code: RpcErrorCode, message: impl Into<String>) -> HostRpcError {
    crate::error::logged_bad_request(code, message)
}

pub(super) fn operational_error(code: RpcErrorCode, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(
        code = code.wire_value(),
        %message,
        "daemon port forward operation failed"
    );
    crate::error::rpc_error(502, code, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionCloseMode {
    GracefulClose,
    RetryableDetach,
    TerminalFailure,
}

pub(super) fn is_recoverable_pressure_error(error: &HostRpcError) -> bool {
    error.code() == Some(RpcErrorCode::PortTunnelLimitExceeded)
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::RpcErrorCode;

    use super::{operational_error, request_error};

    #[test]
    fn request_error_stays_bad_request() {
        let error = request_error(RpcErrorCode::InvalidPortTunnel, "bad tunnel");
        assert_eq!(error.status, 400);
        assert_eq!(error.code, RpcErrorCode::InvalidPortTunnel.wire_value());
    }

    #[test]
    fn operational_error_uses_server_side_status() {
        let error = operational_error(RpcErrorCode::PortConnectFailed, "connect failed");
        assert_eq!(error.status, 502);
        assert_eq!(error.code, RpcErrorCode::PortConnectFailed.wire_value());
    }
}
