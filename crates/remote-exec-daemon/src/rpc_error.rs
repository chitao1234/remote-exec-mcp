use axum::Json;
use axum::http::StatusCode;
use remote_exec_host::HostRpcError;
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

pub(crate) fn bad_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error(RpcErrorCode::BadRequest, message)
}

pub(crate) fn host_rpc_error_response(err: HostRpcError) -> (StatusCode, Json<RpcErrorBody>) {
    let (status, body) = err.into_http_rpc_parts("daemon");
    (
        StatusCode::from_u16(status).expect("normalized HostRpcError status is valid"),
        Json(body),
    )
}

pub(crate) fn domain_error_response<E>(err: E) -> (StatusCode, Json<RpcErrorBody>)
where
    E: Into<HostRpcError>,
{
    host_rpc_error_response(err.into())
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::RpcErrorCode;

    use super::host_rpc_error_response;
    use super::*;

    #[test]
    fn invalid_host_status_falls_back_to_internal_server_error() {
        let (status, body) = host_rpc_error_response(remote_exec_host::HostRpcError::new(
            42,
            RpcErrorCode::Internal,
            "invalid status",
        ));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.0.wire_code(), "internal_error");
        assert_eq!(body.0.message, "invalid status");
    }
}
