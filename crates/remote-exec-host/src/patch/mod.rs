mod engine;
mod matcher;
pub mod parser;
mod preflight;
mod verify;

use std::sync::Arc;

use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse, RpcErrorCode};

use crate::{
    AppState, HostRpcError,
    error::{PatchError, logged_bad_request},
};

const LF: &str = "\n";
const CRLF: &str = "\r\n";

#[derive(Debug)]
pub struct PatchApplyFailure {
    pub error: HostRpcError,
    pub updated_paths: Vec<String>,
}

pub async fn apply_patch_local(
    state: Arc<AppState>,
    req: PatchApplyRequest,
) -> Result<PatchApplyResponse, HostRpcError> {
    apply_patch_local_detailed(state, req)
        .await
        .map_err(|failure| failure.error)
}

pub async fn apply_patch_local_detailed(
    state: Arc<AppState>,
    req: PatchApplyRequest,
) -> Result<PatchApplyResponse, PatchApplyFailure> {
    tracing::info!(
        target = %state.config.target,
        patch_len = req.patch.len(),
        has_workdir = req.workdir.is_some(),
        "patch_apply received"
    );
    let resolved_cwd = crate::exec::resolve_workdir_for_operation(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)
        .map_err(PatchApplyFailure::without_updates)?;
    let cwd = resolved_cwd.into_path_buf();
    let parsed = parser::parse_patch(&req.patch)
        .map_err(|err| logged_bad_request(RpcErrorCode::PatchFailed, err.to_string()))
        .map_err(PatchApplyFailure::without_updates)?;
    let planned = preflight::plan_actions(&state, &cwd, parsed.actions)
        .await
        .map_err(HostRpcError::from)
        .map_err(PatchApplyFailure::without_updates)?;
    let summary = execute_actions(planned)
        .await
        .map_err(|failure| PatchApplyFailure {
            error: failure.error.into(),
            updated_paths: failure.updated_paths,
        })?;
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
        daemon_instance_id: Some(state.daemon_instance_id.clone()),
        updated_paths: summary,
        environment_id: parsed.environment_id,
    })
}

impl PatchApplyFailure {
    fn without_updates(error: HostRpcError) -> Self {
        Self {
            error,
            updated_paths: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct ExecuteActionsFailure {
    error: PatchError,
    updated_paths: Vec<String>,
}

async fn execute_actions(
    actions: Vec<preflight::PlannedAction>,
) -> Result<Vec<String>, ExecuteActionsFailure> {
    let mut summary = Vec::with_capacity(actions.len());

    for action in actions {
        let (description, result): (String, Result<String, PatchError>) = match action {
            preflight::PlannedAction::Add {
                path,
                content,
                summary_path,
            } => {
                let description = format!("add `{summary_path}`");
                let result = async {
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&path, content).await?;
                    Ok(format!("A {summary_path}"))
                }
                .await;
                (description, result)
            }
            preflight::PlannedAction::Delete { path, summary_path } => {
                let description = format!("delete `{summary_path}`");
                let result = async {
                    tokio::fs::remove_file(&path).await?;
                    Ok(format!("D {summary_path}"))
                }
                .await;
                (description, result)
            }
            preflight::PlannedAction::Update {
                source_path,
                destination_path,
                content,
                summary_path,
                remove_source,
            } => {
                let description = if remove_source {
                    format!(
                        "move `{}` to `{}`",
                        source_path.display(),
                        destination_path.display()
                    )
                } else {
                    format!("update `{summary_path}`")
                };
                let result = async {
                    if let Some(parent) = destination_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&destination_path, content).await?;
                    if remove_source {
                        tokio::fs::remove_file(&source_path).await?;
                    }
                    Ok(format!("M {summary_path}"))
                }
                .await;
                (description, result)
            }
        };

        match result {
            Ok(updated_path) => summary.push(updated_path),
            Err(error) => {
                return Err(ExecuteActionsFailure {
                    error: error.with_context(format!("failed to {description}")),
                    updated_paths: summary,
                });
            }
        }
    }

    Ok(summary)
}

fn detect_line_ending(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    for idx in 0..bytes.len() {
        if bytes[idx] != b'\n' {
            continue;
        }
        return if idx > 0 && bytes[idx - 1] == b'\r' {
            CRLF
        } else {
            LF
        };
    }

    LF
}

pub(super) fn ensure_trailing_newline(mut text: String, line_ending: &str) -> String {
    if !text.ends_with('\n') {
        text.push_str(line_ending);
    }
    text
}
