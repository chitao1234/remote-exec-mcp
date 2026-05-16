use std::sync::Arc;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::post;
use axum::{BoxError, Json, Router};
use remote_exec_proto::rpc::RpcErrorBody;
use tower::ServiceBuilder;

use crate::AppState;
use crate::config::DaemonConfig;

pub(crate) fn router(state: Arc<AppState>, daemon_config: Arc<DaemonConfig>) -> Router {
    let request_timeout = daemon_config.request_timeout();

    let timed_routes = Router::new()
        .route("/v1/health", post(crate::server::health))
        .route("/v1/target-info", post(crate::server::target_info))
        .route("/v1/exec/start", post(crate::exec::exec_start))
        .route("/v1/exec/write", post(crate::exec::exec_write))
        .route("/v1/patch/apply", post(crate::patch::apply_patch))
        .route("/v1/transfer/path-info", post(crate::transfer::path_info))
        .route("/v1/transfer/export", post(crate::transfer::export_path))
        .route("/v1/transfer/import", post(crate::transfer::import_archive))
        .route("/v1/image/read", post(crate::image::read_image))
        .layer(
            ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(
                    handle_timeout_error,
                ))
                .timeout(request_timeout),
        );

    let tunnel_route = Router::new().route("/v1/port/tunnel", post(crate::port_forward::tunnel));

    timed_routes
        .merge(tunnel_route)
        .layer(middleware::from_fn(super::version::require_http_11))
        .layer(middleware::from_fn_with_state(
            daemon_config,
            super::auth::require_http_auth,
        ))
        .with_state(state)
        .layer(middleware::from_fn(super::request_log::log_http_request))
}

async fn handle_timeout_error(err: BoxError) -> (StatusCode, Json<RpcErrorBody>) {
    if err.is::<tower::timeout::error::Elapsed>() {
        (
            StatusCode::REQUEST_TIMEOUT,
            Json(RpcErrorBody::from_raw_code(
                "request_timeout",
                "request timed out",
            )),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RpcErrorBody::from_raw_code(
                "internal_error",
                format!("unhandled internal error: {err}"),
            )),
        )
    }
}
