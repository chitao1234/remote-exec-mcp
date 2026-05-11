use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::middleware::Next;
use axum::{
    Json,
    response::{IntoResponse, Response},
};
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

use crate::config::DaemonConfig;

pub async fn require_http_auth(
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
    let expected = http_auth.authorization_header_value();
    if actual == Some(expected.as_str()) {
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, "Bearer")],
        Json(RpcErrorBody::new(
            RpcErrorCode::Unauthorized,
            "missing or invalid bearer token",
        )),
    )
        .into_response()
}
