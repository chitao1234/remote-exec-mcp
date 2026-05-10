use axum::Json;
use axum::http::StatusCode;
use remote_exec_host::HostRpcError;
use remote_exec_proto::rpc::RpcErrorBody;

pub(crate) fn bad_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error("bad_request", message)
}

pub(crate) fn host_rpc_error_response(err: HostRpcError) -> (StatusCode, Json<RpcErrorBody>) {
    let (status, body) = err.into_rpc_parts();
    (
        StatusCode::from_u16(status).expect("valid host rpc status"),
        Json(body),
    )
}
