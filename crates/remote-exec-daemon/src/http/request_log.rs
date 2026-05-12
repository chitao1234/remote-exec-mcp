use std::time::Instant;

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use remote_exec_proto::request_id::{REQUEST_ID_HEADER, RequestId};

pub async fn log_http_request(request: Request, next: Next) -> Response {
    let request_id = request_id_from_headers(&request);
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = Instant::now();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(request_id.as_str()).expect("request id should be a valid header"),
    );
    let status = response.status();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if status.is_server_error() {
        tracing::error!(request_id = %request_id, %method, path = %path, status = status.as_u16(), elapsed_ms, "http request completed");
    } else if status.is_client_error() {
        tracing::warn!(request_id = %request_id, %method, path = %path, status = status.as_u16(), elapsed_ms, "http request completed");
    } else {
        tracing::info!(request_id = %request_id, %method, path = %path, status = status.as_u16(), elapsed_ms, "http request completed");
    }

    response
}

fn request_id_from_headers(request: &Request) -> RequestId {
    request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(RequestId::from_header_value)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware;
    use axum::routing::post;
    use remote_exec_proto::request_id::{REQUEST_ID_HEADER, RequestId};
    use tower::ServiceExt;

    async fn ok() -> &'static str {
        "ok"
    }

    fn app() -> Router {
        Router::new()
            .route("/ok", post(ok))
            .layer(middleware::from_fn(super::log_http_request))
    }

    #[tokio::test]
    async fn log_http_request_echoes_valid_request_id() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ok")
                    .header(REQUEST_ID_HEADER, "client-req-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response
                .headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("client-req-123")
        );
    }

    #[tokio::test]
    async fn log_http_request_generates_missing_request_id() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("request id should be present");
        assert!(RequestId::from_header_value(request_id).is_some());
    }
}
