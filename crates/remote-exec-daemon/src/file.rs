use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use remote_exec_proto::rpc::{
    FileEditRequest, FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};

use crate::AppState;
use crate::rpc_error::RpcError;

pub async fn read_file(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileReadRequest>,
) -> Result<Json<FileReadResponse>, RpcError> {
    crate::rpc_error::domain_json_response(
        remote_exec_host::file::read_file_local(state, req).await,
    )
}

pub async fn write_file(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileWriteRequest>,
) -> Result<Json<FileWriteResponse>, RpcError> {
    crate::rpc_error::domain_json_response(
        remote_exec_host::file::write_file_local(state, req).await,
    )
}

pub async fn edit_file(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileEditRequest>,
) -> Result<Json<FileEditResponse>, RpcError> {
    crate::rpc_error::domain_json_response(
        remote_exec_host::file::edit_file_local(state, req).await,
    )
}
