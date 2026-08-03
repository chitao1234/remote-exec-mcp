mod engine;
mod matcher;
pub mod parser;
mod verify;

use std::sync::Arc;
use std::{io::Write as _, path::Path};

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
    let parsed = parser::parse_patch(&req.patch)
        .map_err(|err| logged_bad_request(RpcErrorCode::PatchFailed, err.to_string()))
        .map_err(PatchApplyFailure::without_updates)?;
    let resolved_cwd = crate::exec::resolve_workdir_for_operation(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)
        .map_err(PatchApplyFailure::without_updates)?;
    let cwd = resolved_cwd.into_path_buf();
    let summary = execute_actions(&state, &cwd, parsed.actions)
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
    state: &Arc<AppState>,
    cwd: &Path,
    actions: Vec<parser::PatchAction>,
) -> Result<Vec<String>, ExecuteActionsFailure> {
    let mut summary = Vec::with_capacity(actions.len());

    for action in actions {
        let description = action_description(&action);
        let result = execute_action(state, cwd, action).await;

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

fn action_description(action: &parser::PatchAction) -> String {
    match action {
        parser::PatchAction::Add { path, .. } => format!("add `{}`", path.display()),
        parser::PatchAction::Delete { path } => format!("delete `{}`", path.display()),
        parser::PatchAction::Update {
            path,
            move_to: Some(destination),
            ..
        } => format!("move `{}` to `{}`", path.display(), destination.display()),
        parser::PatchAction::Update { path, .. } => format!("update `{}`", path.display()),
    }
}

async fn execute_action(
    state: &Arc<AppState>,
    cwd: &Path,
    action: parser::PatchAction,
) -> Result<String, PatchError> {
    match action {
        parser::PatchAction::Add { path, lines } => add_file(state, cwd, path, lines).await,
        parser::PatchAction::Delete { path } => delete_file(state, cwd, path).await,
        parser::PatchAction::Update {
            path,
            move_to,
            hunks,
        } => update_file(state, cwd, path, move_to, hunks).await,
    }
}

async fn add_file(
    state: &Arc<AppState>,
    cwd: &Path,
    path: std::path::PathBuf,
    lines: Vec<String>,
) -> Result<String, PatchError> {
    let resolved_path = verify::resolve_patch_path(state, cwd, &path);
    crate::exec::ensure_resolved_sandbox_access(
        state,
        crate::sandbox::SandboxAccess::Write,
        &resolved_path,
    )?;
    let path = resolved_path.path();
    let summary_path = verify::display_relative(cwd, path);
    let text = ensure_trailing_newline(lines.join("\n"), LF);
    let content = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => {
            let existing = crate::text_file::TextFile::read(
                path,
                state
                    .config
                    .experimental_apply_patch_target_encoding_autodetect,
            )
            .await?;
            existing.encode(&text)?
        }
        Ok(_) => {
            return Err(PatchError::failed(format!(
                "`{summary_path}` is not a file"
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => text.into_bytes(),
        Err(error) => return Err(error.into()),
    };

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    Ok(format!("A {summary_path}"))
}

async fn delete_file(
    state: &Arc<AppState>,
    cwd: &Path,
    path: std::path::PathBuf,
) -> Result<String, PatchError> {
    let resolved_path = verify::resolve_patch_path(state, cwd, &path);
    crate::exec::ensure_resolved_sandbox_access(
        state,
        crate::sandbox::SandboxAccess::Write,
        &resolved_path,
    )?;
    let path = resolved_path.path();
    let summary_path = verify::display_relative(cwd, path);
    tokio::fs::remove_file(path).await?;
    Ok(format!("D {summary_path}"))
}

async fn update_file(
    state: &Arc<AppState>,
    cwd: &Path,
    path: std::path::PathBuf,
    move_to: Option<std::path::PathBuf>,
    hunks: Vec<parser::UpdateChunk>,
) -> Result<String, PatchError> {
    let resolved_source_path = verify::resolve_patch_path(state, cwd, &path);
    crate::exec::ensure_resolved_sandbox_access(
        state,
        crate::sandbox::SandboxAccess::Write,
        &resolved_source_path,
    )?;
    let source_path = resolved_source_path.path().to_path_buf();
    let current = crate::text_file::TextFile::read(
        &source_path,
        state
            .config
            .experimental_apply_patch_target_encoding_autodetect,
    )
    .await?;
    let line_ending = detect_line_ending(&current.text);
    let text = ensure_trailing_newline(
        engine::apply_hunks(&current.text, &hunks, line_ending)?,
        line_ending,
    );
    let content = current.encode(&text)?;

    let destination_path = move_to
        .as_ref()
        .map(|destination| verify::resolve_patch_path(state, cwd, destination))
        .map(|destination| destination.into_path_buf())
        .unwrap_or_else(|| source_path.clone());
    let remove_source =
        move_to.is_some() && !crate::path_compare::path_eq(&source_path, &destination_path);
    let summary_path = verify::display_relative(cwd, &destination_path);

    if !remove_source {
        atomic_replace(destination_path, content).await?;
        return Ok(format!("M {summary_path}"));
    }

    let resolved_destination_path = verify::resolve_patch_path(
        state,
        cwd,
        move_to
            .as_ref()
            .expect("move destination exists when source is removed"),
    );
    crate::exec::ensure_resolved_sandbox_access(
        state,
        crate::sandbox::SandboxAccess::Write,
        &resolved_destination_path,
    )?;
    if let Some(parent) = destination_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&destination_path, content).await?;
    tokio::fs::remove_file(&source_path).await?;
    Ok(format!("M {summary_path}"))
}

async fn atomic_replace(path: std::path::PathBuf, content: Vec<u8>) -> Result<(), PatchError> {
    let parent = path.parent().map(Path::to_path_buf).ok_or_else(|| {
        PatchError::failed(format!("`{}` has no parent directory", path.display()))
    })?;

    tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
        let existing_permissions = match std::fs::metadata(&path) {
            Ok(metadata) => Some(metadata.permissions()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&content)?;
        if let Some(permissions) = existing_permissions {
            temporary.as_file().set_permissions(permissions)?;
        }
        temporary
            .persist(path)
            .map(|_| ())
            .map_err(|error| error.error)
    })
    .await
    .map_err(|error| PatchError::internal(format!("atomic patch write task failed: {error}")))??;

    Ok(())
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
