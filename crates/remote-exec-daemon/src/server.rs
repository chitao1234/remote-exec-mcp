use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{HealthCheckResponse, TargetInfoResponse};

use crate::AppState;

pub async fn serve(state: AppState) -> Result<()> {
    serve_with_shutdown(state, std::future::pending::<()>()).await
}

pub async fn serve_with_shutdown<F>(state: AppState, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let state = Arc::new(state);
    let app = router(state.clone());
    crate::tls::serve_tls_with_shutdown(app, state, shutdown).await
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(crate::exec::exec_start))
        .route("/v1/exec/write", post(crate::exec::exec_write))
        .route("/v1/patch/apply", post(crate::patch::apply_patch))
        .route("/v1/transfer/export", post(crate::transfer::export_path))
        .route("/v1/transfer/import", post(crate::transfer::import_archive))
        .route("/v1/image/read", post(crate::image::read_image))
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthCheckResponse> {
    Json(HealthCheckResponse {
        status: "ok".to_string(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
    })
}

async fn target_info(State(state): State<Arc<AppState>>) -> Json<TargetInfoResponse> {
    Json(crate::target_info_response(&state))
}
