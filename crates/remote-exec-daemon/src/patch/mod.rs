mod engine;
mod matcher;
pub mod parser;
mod verify;

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse, RpcErrorBody};
use remote_exec_proto::sandbox::SandboxError;

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
    tracing::info!(
        target = %state.config.target,
        patch_len = req.patch.len(),
        has_workdir = req.workdir.is_some(),
        "patch_apply received"
    );
    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)?;
    let actions = parser::parse_patch(&req.patch)
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
    let summary = execute_actions(&state, &cwd, actions)
        .await
        .map_err(map_patch_error)?;
    tracing::info!(
        target = %state.config.target,
        updated_paths = summary.len(),
        "patch_apply completed"
    );

    Ok(PatchApplyResponse {
        output: format!(
            "Success. Updated the following files:\n{}\n",
            summary.join("\n")
        ),
    })
}

async fn execute_actions(
    state: &Arc<AppState>,
    cwd: &Path,
    actions: Vec<parser::PatchAction>,
) -> anyhow::Result<Vec<String>> {
    let mut summary = Vec::with_capacity(actions.len());

    for action in actions {
        let verified = verify::verify_action(state, cwd, action).await?;
        match verified {
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
                hunks,
                summary_path,
                remove_source,
            } => {
                let current = tokio::fs::read_to_string(&source_path).await?;
                let content = ensure_trailing_newline(engine::apply_hunks(&current, &hunks)?);
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

fn map_patch_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    let code = if err.downcast_ref::<SandboxError>().is_some() {
        "sandbox_denied"
    } else {
        "patch_failed"
    };
    crate::exec::rpc_error(code, err.to_string())
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}
