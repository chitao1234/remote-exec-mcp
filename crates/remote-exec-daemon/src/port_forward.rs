use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONNECTION, UPGRADE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use remote_exec_proto::port_tunnel::{
    TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN,
};
use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

pub async fn tunnel(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    validate_upgrade_headers(&headers)?;
    let connection_permit = remote_exec_host::port_forward::reserve_tunnel_connection(&state)
        .map_err(crate::rpc_error::host_rpc_error_response)?;
    let on_upgrade = upgrade::on(request);

    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                if let Err(err) = remote_exec_host::port_forward::serve_tunnel_with_permit(
                    state,
                    TokioIo::new(upgraded),
                    connection_permit,
                )
                .await
                {
                    tracing::warn!(error = %err.message, code = %err.code, "port tunnel ended with error");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "port tunnel upgrade failed");
            }
        }
    });

    Ok((
        StatusCode::SWITCHING_PROTOCOLS,
        [(CONNECTION, "Upgrade"), (UPGRADE, UPGRADE_TOKEN)],
        Body::empty(),
    )
        .into_response())
}

fn validate_upgrade_headers(headers: &HeaderMap) -> Result<(), (StatusCode, Json<RpcErrorBody>)> {
    if !header_contains_token(headers, CONNECTION.as_str(), "upgrade") {
        return Err(bad_upgrade_request("missing `Connection: Upgrade` header"));
    }
    if !header_eq(headers, UPGRADE.as_str(), UPGRADE_TOKEN) {
        return Err(bad_upgrade_request(format!(
            "missing `Upgrade: {UPGRADE_TOKEN}` header"
        )));
    }
    if !header_eq(
        headers,
        TUNNEL_PROTOCOL_VERSION_HEADER,
        TUNNEL_PROTOCOL_VERSION,
    ) {
        return Err(bad_upgrade_request(format!(
            "missing `{TUNNEL_PROTOCOL_VERSION_HEADER}: {TUNNEL_PROTOCOL_VERSION}` header"
        )));
    }
    Ok(())
}

fn header_contains_token(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(expected))
}

fn header_eq(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn bad_upgrade_request(message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody::new(RpcErrorCode::BadRequest, message)),
    )
}
