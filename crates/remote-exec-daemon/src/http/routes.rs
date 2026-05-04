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
        .route("/v1/port/listen", post(crate::port_forward::listen))
        .route(
            "/v1/port/listen/accept",
            post(crate::port_forward::listen_accept),
        )
        .route(
            "/v1/port/listen/close",
            post(crate::port_forward::listen_close),
        )
        .route("/v1/port/connect", post(crate::port_forward::connect))
        .route(
            "/v1/port/connection/read",
            post(crate::port_forward::connection_read),
        )
        .route(
            "/v1/port/connection/write",
            post(crate::port_forward::connection_write),
        )
        .route(
            "/v1/port/connection/close",
            post(crate::port_forward::connection_close),
        )
        .route(
            "/v1/port/udp/read",
            post(crate::port_forward::udp_datagram_read),
        )
        .route(
            "/v1/port/udp/write",
            post(crate::port_forward::udp_datagram_write),
        )
        .layer(middleware::from_fn_with_state(
            daemon_config,
            super::auth::require_http_auth,
        ))
        .with_state(state)
        .layer(middleware::from_fn(super::request_log::log_http_request))
}
