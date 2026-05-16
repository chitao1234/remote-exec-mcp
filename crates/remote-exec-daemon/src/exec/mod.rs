use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest, RpcErrorCode};

use crate::rpc_error::RpcError;

pub use remote_exec_host::exec::session;

pub async fn exec_start(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, RpcError> {
    remote_exec_host::exec::exec_start_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn exec_write(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, RpcError> {
    remote_exec_host::exec::exec_write_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub(crate) fn rpc_error(code: RpcErrorCode, message: impl Into<String>) -> RpcError {
    crate::rpc_error::host_rpc_error_response(remote_exec_host::HostRpcError::new(
        400, code, message,
    ))
}

pub(crate) fn internal_error(err: anyhow::Error) -> RpcError {
    crate::rpc_error::host_rpc_error_response(remote_exec_host::exec::internal_error(err))
}
