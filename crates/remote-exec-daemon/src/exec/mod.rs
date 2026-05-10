use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest, RpcErrorBody};

pub use remote_exec_host::exec::session;

pub async fn exec_start(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::exec::exec_start_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn exec_write(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::exec::exec_write_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub(crate) fn rpc_error(
    code: &'static str,
    message: impl Into<String>,
) -> (StatusCode, Json<RpcErrorBody>) {
    crate::rpc_error::host_rpc_error_response(remote_exec_host::exec::rpc_error(code, message))
}

pub(crate) fn internal_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    crate::rpc_error::host_rpc_error_response(remote_exec_host::exec::internal_error(err))
}
