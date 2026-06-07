use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest};

use crate::rpc_error::RpcError;

pub use remote_exec_host::exec::session;

pub async fn exec_start(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, RpcError> {
    crate::rpc_error::host_json_response(remote_exec_host::exec::exec_start_local(state, req).await)
}

pub async fn exec_write(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, RpcError> {
    crate::rpc_error::host_json_response(remote_exec_host::exec::exec_write_local(state, req).await)
}
