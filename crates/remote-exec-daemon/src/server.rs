use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use remote_exec_proto::rpc::{HealthCheckResponse, TargetInfoResponse};

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
    let listener = crate::tls::bind_listener(daemon_config.listen)?;
    serve_with_shutdown_on_listener(state, daemon_config, listener, shutdown).await
}

pub(crate) async fn serve_with_shutdown_on_listener<F>(
    state: AppState,
    daemon_config: Arc<DaemonConfig>,
    listener: tokio::net::TcpListener,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let state = Arc::new(state);
    let app = crate::http::routes::router(state.clone(), daemon_config.clone());
    let result =
        crate::tls::serve_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await;
    state.background_tasks.join_all().await;
    result
}

pub(crate) async fn health(State(state): State<Arc<AppState>>) -> Json<HealthCheckResponse> {
    Json(HealthCheckResponse {
        status: "ok".to_string(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
    })
}

pub(crate) async fn target_info(State(state): State<Arc<AppState>>) -> Json<TargetInfoResponse> {
    Json(crate::target_info_response(&state))
}
