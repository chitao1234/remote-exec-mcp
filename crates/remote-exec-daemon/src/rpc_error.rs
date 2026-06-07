use axum::Json;
use axum::http::StatusCode;
use remote_exec_host::HostRpcError;
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

pub(crate) type RpcError = (StatusCode, Json<RpcErrorBody>);

pub(crate) fn bad_request(message: impl Into<String>) -> RpcError {
    host_rpc_error_response(HostRpcError::new(400, RpcErrorCode::BadRequest, message))
}

pub(crate) fn host_rpc_error_response(err: HostRpcError) -> RpcError {
    let (status, body) = err.into_http_rpc_parts("daemon");
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(body),
    )
}

pub(crate) fn domain_error_response<E>(err: E) -> RpcError
where
    E: Into<HostRpcError>,
{
    host_rpc_error_response(err.into())
}

pub(crate) fn host_response<T>(result: Result<T, HostRpcError>) -> Result<T, RpcError> {
    result.map_err(host_rpc_error_response)
}

pub(crate) fn domain_response<T, E>(result: Result<T, E>) -> Result<T, RpcError>
where
    E: Into<HostRpcError>,
{
    result.map_err(domain_error_response)
}

pub(crate) fn host_json_response<T>(result: Result<T, HostRpcError>) -> Result<Json<T>, RpcError> {
    host_response(result).map(Json)
}

pub(crate) fn domain_json_response<T, E>(result: Result<T, E>) -> Result<Json<T>, RpcError>
where
    E: Into<HostRpcError>,
{
    domain_response(result).map(Json)
}

pub(crate) fn internal_response(err: anyhow::Error) -> RpcError {
    host_rpc_error_response(remote_exec_host::exec::internal_error(err))
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
