use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse};

use crate::AppState;
use crate::rpc_error::RpcError;

pub async fn read_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, RpcError> {
    crate::rpc_error::domain_json_response(
        remote_exec_host::image::read_image_local(state, req).await,
    )
}
