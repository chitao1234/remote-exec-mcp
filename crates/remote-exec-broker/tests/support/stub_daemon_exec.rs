use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use remote_exec_proto::rpc::{
    ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecRunningResponse, ExecStartRequest,
    ExecWriteRequest, RpcErrorBody,
};

use super::StubDaemonState;

#[derive(Debug, Clone, Copy)]
pub enum ExecWriteBehavior {
    Success,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    TemporaryFailureOnce,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    UnknownSession,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    MalformedCompletedMissingExitCode,
}

#[derive(Debug, Clone, Copy)]
pub enum ExecStartBehavior {
    Success,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    RunningMissingDaemonSessionId,
}

pub(crate) async fn set_exec_start_behavior(state: &StubDaemonState, behavior: ExecStartBehavior) {
    *state.exec_start_behavior.lock().await = behavior;
}

pub(crate) async fn set_exec_write_behavior(state: &StubDaemonState, behavior: ExecWriteBehavior) {
    *state.exec_write_behavior.lock().await = behavior;
}

pub(super) async fn exec_start(
    State(state): State<StubDaemonState>,
    Json(_req): Json<ExecStartRequest>,
) -> Response {
    *state.exec_start_calls.lock().await += 1;
    let behavior = *state.exec_start_behavior.lock().await;
    let warnings = state.exec_start_warnings.lock().await.clone();
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();
    let daemon_session_id = state.daemon_session_id.lock().await.clone();

    let body = match behavior {
        ExecStartBehavior::Success => {
            serde_json::to_value(ExecResponse::Running(ExecRunningResponse {
                daemon_session_id,
                output: ExecOutputResponse {
                    daemon_instance_id,
                    running: true,
                    chunk_id: Some("chunk-start".to_string()),
                    wall_time_seconds: 0.25,
                    exit_code: None,
                    original_token_count: Some(1),
                    output: "ready".to_string(),
                    warnings,
                },
            }))
            .unwrap()
        }
        ExecStartBehavior::RunningMissingDaemonSessionId => serde_json::json!({
            "daemon_instance_id": daemon_instance_id,
            "running": true,
            "chunk_id": "chunk-start",
            "wall_time_seconds": 0.25,
            "exit_code": null,
            "original_token_count": 1,
            "output": "ready",
            "warnings": warnings,
        }),
    };

    (StatusCode::OK, Json(body)).into_response()
}

pub(super) async fn exec_write(
    State(state): State<StubDaemonState>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)> {
    let expected_daemon_session_id = state.daemon_session_id.lock().await.clone();
    if req.daemon_session_id != expected_daemon_session_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "unknown_session".to_string(),
                message: "Unknown daemon session".to_string(),
            }),
        ));
    }

    *state.last_exec_write_request.lock().await = Some(req.clone());
    let mut behavior = state.exec_write_behavior.lock().await;
    let response_behavior = *behavior;
    match *behavior {
        ExecWriteBehavior::Success => {}
        ExecWriteBehavior::TemporaryFailureOnce => {
            *behavior = ExecWriteBehavior::Success;
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RpcErrorBody {
                    code: "temporary_failure".to_string(),
                    message: "temporary failure".to_string(),
                }),
            ));
        }
        ExecWriteBehavior::UnknownSession => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(RpcErrorBody {
                    code: "unknown_session".to_string(),
                    message: "Unknown daemon session".to_string(),
                }),
            ));
        }
        ExecWriteBehavior::MalformedCompletedMissingExitCode => {}
    }
    drop(behavior);
    let daemon_instance_id = state.daemon_instance_id.lock().await.clone();

    let body = match response_behavior {
        ExecWriteBehavior::MalformedCompletedMissingExitCode => serde_json::json!({
            "daemon_session_id": null,
            "daemon_instance_id": daemon_instance_id,
            "running": false,
            "chunk_id": "chunk-write",
            "wall_time_seconds": 0.5,
            "exit_code": null,
            "original_token_count": 2,
            "output": "poll output",
            "warnings": [],
        }),
        _ => serde_json::to_value(ExecResponse::Completed(ExecCompletedResponse {
            output: ExecOutputResponse {
                daemon_instance_id,
                running: false,
                chunk_id: Some("chunk-write".to_string()),
                wall_time_seconds: 0.5,
                exit_code: Some(0),
                original_token_count: Some(2),
                output: "poll output".to_string(),
                warnings: Vec::new(),
            },
        }))
        .unwrap(),
    };

    Ok((StatusCode::OK, Json(body)).into_response())
}
