use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse};

use crate::rpc_error::RpcError;

pub async fn apply_patch(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, RpcError> {
    remote_exec_host::patch::apply_patch_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}
