use axum::Json;
use axum::extract::Request;
use axum::http::{StatusCode, Version};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use remote_exec_proto::rpc::RpcErrorBody;

pub async fn require_http_11(request: Request, next: Next) -> Response {
    if request.version() == Version::HTTP_11 {
        return next.run(request).await;
    }

    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code: "bad_request".to_string(),
            message: "only HTTP/1.1 is supported".to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, Version};
    use axum::middleware;
    use axum::routing::post;
    use tower::ServiceExt;

    use super::require_http_11;

    async fn ok() -> &'static str {
        "ok"
    }

    fn request_with_version(version: Version) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri("/ok")
            .version(version)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn require_http_11_allows_http_11() {
        let app = Router::new()
            .route("/ok", post(ok))
            .layer(middleware::from_fn(require_http_11));

        let response = app
            .oneshot(request_with_version(Version::HTTP_11))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn require_http_11_rejects_other_versions() {
        let app = Router::new()
            .route("/ok", post(ok))
            .layer(middleware::from_fn(require_http_11));

        let response = app
            .oneshot(request_with_version(Version::HTTP_10))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
