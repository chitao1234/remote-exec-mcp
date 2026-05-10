use axum::Json;
use axum::http::StatusCode;
use remote_exec_host::HostRpcError;
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

pub(crate) fn bad_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error(RpcErrorCode::BadRequest, message)
}

pub(crate) fn host_rpc_error_response(err: HostRpcError) -> (StatusCode, Json<RpcErrorBody>) {
    let (status, body) = err.into_rpc_parts();
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(body),
    )
}

pub(crate) fn domain_error_response<E>(err: E) -> (StatusCode, Json<RpcErrorBody>)
where
    E: Into<HostRpcError>,
{
    host_rpc_error_response(err.into())
}
