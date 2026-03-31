pub mod parser;

use std::path::Path;
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
    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)?;
    let actions = parser::parse_patch(&req.patch)
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
    let mut summary = Vec::new();

    for action in actions {
        match action {
            parser::PatchAction::Add { path, lines } => {
                let path = cwd.join(path);
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                }
                tokio::fs::write(&path, format!("{}\n", lines.join("\n")))
                    .await
                    .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                summary.push(format!("A {}", display_relative(&cwd, &path)));
            }
            parser::PatchAction::Delete { path } => {
                let path = cwd.join(path);
                tokio::fs::remove_file(&path)
                    .await
                    .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                summary.push(format!("D {}", display_relative(&cwd, &path)));
            }
            parser::PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let path = cwd.join(path);
                let current = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                let updated = apply_hunks(&current, &hunks)
                    .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;

                if let Some(move_to) = move_to {
                    let destination = cwd.join(move_to);
                    if let Some(parent) = destination.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|err| {
                            crate::exec::rpc_error("patch_failed", err.to_string())
                        })?;
                    }
                    tokio::fs::write(&destination, ensure_trailing_newline(updated))
                        .await
                        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                    tokio::fs::remove_file(&path)
                        .await
                        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                    summary.push(format!("M {}", display_relative(&cwd, &destination)));
                } else {
                    tokio::fs::write(&path, ensure_trailing_newline(updated))
                        .await
                        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
                    summary.push(format!("M {}", display_relative(&cwd, &path)));
                }
            }
        }
    }

    Ok(Json(PatchApplyResponse {
        output: format!("Success. Updated the following files:\n{}\n", summary.join("\n")),
    }))
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn display_relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).display().to_string()
}

fn apply_hunks(current: &str, hunks: &[parser::Hunk]) -> anyhow::Result<String> {
    let mut lines = current.lines().map(str::to_string).collect::<Vec<_>>();

    for hunk in hunks {
        let anchor = hunk
            .context
            .as_ref()
            .and_then(|ctx| lines.iter().position(|line| line.contains(ctx)));
        let mut cursor = anchor.unwrap_or(0);
        let mut replacement = Vec::new();

        for line in &hunk.lines {
            match line {
                parser::HunkLine::Context(value) => {
                    let found = lines
                        .iter()
                        .enumerate()
                        .skip(cursor)
                        .find(|(_, line)| *line == value)
                        .map(|(index, _)| index)
                        .ok_or_else(|| anyhow::anyhow!("context line `{value}` not found"))?;
                    replacement.extend(lines[cursor..=found].iter().cloned());
                    cursor = found + 1;
                }
                parser::HunkLine::Delete(value) => {
                    let found = lines
                        .iter()
                        .enumerate()
                        .skip(cursor)
                        .find(|(_, line)| *line == value)
                        .map(|(index, _)| index)
                        .ok_or_else(|| anyhow::anyhow!("delete line `{value}` not found"))?;
                    replacement.extend(lines[cursor..found].iter().cloned());
                    cursor = found + 1;
                }
                parser::HunkLine::Add(value) => replacement.push(value.clone()),
            }
        }

        replacement.extend(lines[cursor..].iter().cloned());
        lines = replacement;
    }

    Ok(lines.join("\n"))
}
