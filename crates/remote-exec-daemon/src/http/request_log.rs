use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

pub async fn log_http_request(request: Request, next: Next) -> Response {
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
