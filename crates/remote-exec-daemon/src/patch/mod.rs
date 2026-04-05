mod engine;
pub mod parser;
mod verify;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse, RpcErrorBody};

use crate::AppState;

pub async fn apply_patch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    apply_patch_local(state, req).await.map(Json)
}

pub async fn apply_patch_local(
    state: Arc<AppState>,
    req: PatchApplyRequest,
) -> Result<PatchApplyResponse, (StatusCode, Json<RpcErrorBody>)> {
    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)?;
    let actions = parser::parse_patch(&req.patch)
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
    let verified = verify::verify_actions(&cwd, actions)
        .await
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
    let summary = execute_verified_actions(verified)
        .await
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;

    Ok(PatchApplyResponse {
        output: format!(
            "Success. Updated the following files:\n{}\n",
            summary.join("\n")
        ),
    })
}

async fn execute_verified_actions(
    actions: Vec<verify::VerifiedAction>,
) -> anyhow::Result<Vec<String>> {
    let mut summary = Vec::with_capacity(actions.len());

    for action in actions {
        match action {
            verify::VerifiedAction::Add {
                path,
                content,
                summary_path,
            } => {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, content).await?;
                summary.push(format!("A {summary_path}"));
            }
            verify::VerifiedAction::Delete { path, summary_path } => {
                tokio::fs::remove_file(&path).await?;
                summary.push(format!("D {summary_path}"));
            }
            verify::VerifiedAction::Update {
                source_path,
                destination_path,
                content,
                summary_path,
                remove_source,
            } => {
                if let Some(parent) = destination_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&destination_path, content).await?;
                if remove_source {
                    tokio::fs::remove_file(&source_path).await?;
                }
                summary.push(format!("M {summary_path}"));
            }
        }
    }

    Ok(summary)
}
