use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing::post;

use crate::AppState;
use crate::config::DaemonConfig;

pub(crate) fn router(state: Arc<AppState>, daemon_config: Arc<DaemonConfig>) -> Router {
    Router::new()
        .route("/v1/health", post(crate::server::health))
        .route("/v1/target-info", post(crate::server::target_info))
        .route("/v1/exec/start", post(crate::exec::exec_start))
        .route("/v1/exec/write", post(crate::exec::exec_write))
        .route("/v1/patch/apply", post(crate::patch::apply_patch))
        .route("/v1/transfer/path-info", post(crate::transfer::path_info))
        .route("/v1/transfer/export", post(crate::transfer::export_path))
        .route("/v1/transfer/import", post(crate::transfer::import_archive))
        .route("/v1/image/read", post(crate::image::read_image))
        .route("/v1/port/tunnel", post(crate::port_forward::tunnel))
        .layer(middleware::from_fn(super::version::require_http_11))
        .layer(middleware::from_fn_with_state(
            daemon_config,
            super::auth::require_http_auth,
        ))
        .with_state(state)
        .layer(middleware::from_fn(super::request_log::log_http_request))
}
