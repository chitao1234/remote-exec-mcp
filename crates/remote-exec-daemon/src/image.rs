use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse, RpcErrorBody};

use crate::AppState;
use crate::rpc_error::host_rpc_error_response;

pub async fn read_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::image::read_image_local(state, req)
        .await
        .map(Json)
        .map_err(|err| host_rpc_error_response(err.into_host_rpc_error()))
}
