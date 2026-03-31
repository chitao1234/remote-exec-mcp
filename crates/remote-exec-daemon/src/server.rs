use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{HealthCheckResponse, TargetInfoResponse};

use crate::AppState;

pub async fn serve(state: AppState) -> Result<()> {
    let state = Arc::new(state);
    let app = router(state.clone());
    crate::tls::serve_tls(app, state).await
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(crate::exec::exec_start))
        .route("/v1/exec/write", post(crate::exec::exec_write))
        .route("/v1/patch/apply", post(crate::patch::apply_patch))
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
    Json(TargetInfoResponse {
        target: state.config.target.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        supports_pty: true,
        supports_image_read: true,
    })
}
