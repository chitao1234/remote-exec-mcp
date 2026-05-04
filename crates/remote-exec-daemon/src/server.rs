use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::middleware::{self, Next};
use axum::routing::post;
use axum::{
    Json, Router,
    response::{IntoResponse, Response},
};
use remote_exec_proto::rpc::{HealthCheckResponse, RpcErrorBody, TargetInfoResponse};

use crate::AppState;
use crate::config::DaemonConfig;

pub async fn serve(state: AppState, daemon_config: Arc<DaemonConfig>) -> Result<()> {
    serve_with_shutdown(state, daemon_config, std::future::pending::<()>()).await
}

pub async fn serve_with_shutdown<F>(
    state: AppState,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let state = Arc::new(state);
    let app = router(state.clone(), daemon_config.clone());
    crate::tls::serve_with_shutdown(app, daemon_config, shutdown).await
}

pub fn router(state: Arc<AppState>, daemon_config: Arc<DaemonConfig>) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
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
            require_http_auth,
        ))
        .with_state(state)
        .layer(middleware::from_fn(log_http_request))
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

async fn log_http_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if status.is_server_error() {
        tracing::error!(%method, path = %path, status = status.as_u16(), elapsed_ms, "http request completed");
    } else if status.is_client_error() {
        tracing::warn!(%method, path = %path, status = status.as_u16(), elapsed_ms, "http request completed");
    } else {
        tracing::info!(%method, path = %path, status = status.as_u16(), elapsed_ms, "http request completed");
    }

    response
}

async fn require_http_auth(
    State(daemon_config): State<Arc<DaemonConfig>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(http_auth) = daemon_config.http_auth.as_ref() else {
        return next.run(request).await;
    };

    let actual = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let expected = format!("Bearer {}", http_auth.bearer_token);
    if actual == Some(expected.as_str()) {
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, "Bearer")],
        Json(RpcErrorBody {
            code: "unauthorized".to_string(),
            message: "missing or invalid bearer token".to_string(),
        }),
    )
        .into_response()
}
