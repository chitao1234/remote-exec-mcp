use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{
    FileEditRequest, FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse, RpcErrorBody,
};

use super::StubDaemonState;

#[derive(Debug, Clone)]
pub enum StubFileReadResponse {
    Success(FileReadResponse),
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Debug, Clone)]
pub enum StubFileWriteResponse {
    Success(FileWriteResponse),
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

#[derive(Debug, Clone)]
pub enum StubFileEditResponse {
    Success(FileEditResponse),
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

pub(crate) async fn set_file_read_response(
    state: &StubDaemonState,
    response: StubFileReadResponse,
) {
    *state.file_read_response.lock().await = response;
}

pub(crate) async fn set_file_write_response(
    state: &StubDaemonState,
    response: StubFileWriteResponse,
) {
    *state.file_write_response.lock().await = response;
}

pub(crate) async fn set_file_edit_response(
    state: &StubDaemonState,
    response: StubFileEditResponse,
) {
    *state.file_edit_response.lock().await = response;
}

pub(super) async fn file_read(
    State(state): State<StubDaemonState>,
    Json(req): Json<FileReadRequest>,
) -> Result<Json<FileReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    *state.last_file_read_request.lock().await = Some(req);
    match state.file_read_response.lock().await.clone() {
        StubFileReadResponse::Success(response) => Ok(Json(response)),
        StubFileReadResponse::Error { status, body } => Err((status, Json(body))),
    }
}

pub(super) async fn file_write(
    State(state): State<StubDaemonState>,
    Json(req): Json<FileWriteRequest>,
) -> Result<Json<FileWriteResponse>, (StatusCode, Json<RpcErrorBody>)> {
    *state.last_file_write_request.lock().await = Some(req);
    match state.file_write_response.lock().await.clone() {
        StubFileWriteResponse::Success(response) => Ok(Json(response)),
        StubFileWriteResponse::Error { status, body } => Err((status, Json(body))),
    }
}

pub(super) async fn file_edit(
    State(state): State<StubDaemonState>,
    Json(req): Json<FileEditRequest>,
) -> Result<Json<FileEditResponse>, (StatusCode, Json<RpcErrorBody>)> {
    *state.last_file_edit_request.lock().await = Some(req);
    match state.file_edit_response.lock().await.clone() {
        StubFileEditResponse::Success(response) => Ok(Json(response)),
        StubFileEditResponse::Error { status, body } => Err((status, Json(body))),
    }
}
