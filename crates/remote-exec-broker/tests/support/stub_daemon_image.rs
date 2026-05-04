use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse, RpcErrorBody};

use super::StubDaemonState;

#[derive(Debug, Clone)]
pub enum StubImageReadResponse {
    Success(ImageReadResponse),
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    Error {
        status: StatusCode,
        body: RpcErrorBody,
    },
}

pub(crate) async fn set_image_read_response(
    state: &StubDaemonState,
    response: StubImageReadResponse,
) {
    *state.image_read_response.lock().await = response;
}

pub(super) async fn image_read(
    State(state): State<StubDaemonState>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    match state.image_read_response.lock().await.clone() {
        StubImageReadResponse::Success(mut response) => {
            response.detail = req.detail.filter(|value| value == "original");
            Ok(Json(response))
        }
        StubImageReadResponse::Error { status, body } => Err((status, Json(body))),
    }
}
