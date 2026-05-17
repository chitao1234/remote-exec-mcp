use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;

use super::{
    detect_line_ending, engine, ensure_trailing_newline,
    parser::PatchAction,
    text_codec::PatchTextFile,
    verify::{display_relative, resolve_patch_path},
};
use crate::{AppState, error::PatchError, sandbox::SandboxAccess};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlannedAction {
    Add {
        path: PathBuf,
        content: Vec<u8>,
        summary_path: String,
    },
    Delete {
        path: PathBuf,
        summary_path: String,
    },
    Update {
        source_path: PathBuf,
        destination_path: PathBuf,
        content: Vec<u8>,
        summary_path: String,
        remove_source: bool,
    },
}

#[derive(Debug, Clone)]
struct PlannedFile {
    text: PatchTextFile,
    content: Vec<u8>,
}

#[derive(Debug, Clone)]
enum PlannedPathState {
    File(PlannedFile),
    Deleted,
}

#[derive(Debug, Default)]
struct PathOverlay {
    files: Vec<(PathBuf, PlannedPathState)>,
    directories: Vec<PathBuf>,
}

impl PathOverlay {
    fn get(&self, path: &Path) -> Option<&PlannedPathState> {
        let key = overlay_key(path);
        self.files
            .iter()
            .find(|(candidate, _)| path_eq(candidate, &key))
            .map(|(_, state)| state)
    }

    fn set(&mut self, path: PathBuf, state: PlannedPathState) {
        let key = overlay_key(&path);
        if let Some((_, existing)) = self
            .files
            .iter_mut()
            .find(|(candidate, _)| path_eq(candidate, &key))
        {
            *existing = state;
            return;
        }
        self.files.push((key, state));
    }

    fn mark_parent_directories(&mut self, path: &Path) {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent.as_os_str().is_empty() {
                break;
            }
            self.mark_directory(parent);
            current = parent.parent();
        }
    }

    fn mark_directory(&mut self, path: &Path) {
        let key = overlay_key(path);
        if !self
            .directories
            .iter()
            .any(|candidate| path_eq(candidate, &key))
        {
            self.directories.push(key);
        }
    }

    fn contains_directory(&self, path: &Path) -> bool {
        let key = overlay_key(path);
        self.directories
            .iter()
            .any(|candidate| path_eq(candidate, &key))
    }
}

pub(super) async fn plan_actions(
    state: &Arc<AppState>,
    cwd: &Path,
    actions: Vec<PatchAction>,
) -> Result<Vec<PlannedAction>, PatchError> {
    let mut overlay = PathOverlay::default();
    let mut planned = Vec::with_capacity(actions.len());

    for action in actions {
        let action = plan_action(state, cwd, action, &mut overlay).await?;
        planned.push(action);
    }

    Ok(planned)
}

async fn plan_action(
    state: &Arc<AppState>,
    cwd: &Path,
    action: PatchAction,
    overlay: &mut PathOverlay,
) -> Result<PlannedAction, PatchError> {
    match action {
        PatchAction::Add { path, lines } => plan_add(state, cwd, path, lines, overlay).await,
        PatchAction::Delete { path } => plan_delete(state, cwd, path, overlay).await,
        PatchAction::Update {
            path,
            move_to,
            hunks,
        } => plan_update(state, cwd, path, move_to, hunks, overlay).await,
    }
}

async fn plan_add(
    state: &Arc<AppState>,
    cwd: &Path,
    path: PathBuf,
    lines: Vec<String>,
    overlay: &mut PathOverlay,
) -> Result<PlannedAction, PatchError> {
    let absolute_path = resolve_patch_path(state, cwd, &path);
    crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &absolute_path)?;
    ensure_writable_file_target(cwd, &absolute_path, overlay).await?;

    let text = ensure_trailing_newline(lines.join("\n"), "\n");
    let existing = file_from_overlay_or_disk(state, cwd, &absolute_path, overlay).await?;
    let file = match existing {
        Some(existing) => {
            let content = existing.text.encode(&text)?;
            PlannedFile {
                text: existing.text.with_text(text),
                content,
            }
        }
        None => PlannedFile {
            content: text.as_bytes().to_vec(),
            text: PatchTextFile::utf8(text),
        },
    };

    overlay.mark_parent_directories(&absolute_path);
    overlay.set(absolute_path.clone(), PlannedPathState::File(file.clone()));

    Ok(PlannedAction::Add {
        path: absolute_path.clone(),
        content: file.content,
        summary_path: display_relative(cwd, &absolute_path),
    })
}

async fn plan_delete(
    state: &Arc<AppState>,
    cwd: &Path,
    path: PathBuf,
    overlay: &mut PathOverlay,
) -> Result<PlannedAction, PatchError> {
    let absolute_path = resolve_patch_path(state, cwd, &path);
    crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &absolute_path)?;
    let _ = require_file(state, cwd, &absolute_path, overlay).await?;

    overlay.set(absolute_path.clone(), PlannedPathState::Deleted);

    Ok(PlannedAction::Delete {
        path: absolute_path.clone(),
        summary_path: display_relative(cwd, &absolute_path),
    })
}

async fn plan_update(
    state: &Arc<AppState>,
    cwd: &Path,
    path: PathBuf,
    move_to: Option<PathBuf>,
    hunks: Vec<super::parser::UpdateChunk>,
    overlay: &mut PathOverlay,
) -> Result<PlannedAction, PatchError> {
    let source_path = resolve_patch_path(state, cwd, &path);
    crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &source_path)?;
    let current = require_file(state, cwd, &source_path, overlay).await?;
    let destination_path = move_to
        .as_ref()
        .map(|destination| resolve_patch_path(state, cwd, destination))
        .unwrap_or_else(|| source_path.clone());
    let remove_source = move_to.is_some() && !path_eq(&source_path, &destination_path);
    if remove_source {
        crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &destination_path)?;
        ensure_writable_file_target(cwd, &destination_path, overlay).await?;
    }

    let line_ending = detect_line_ending(&current.text.text);
    let text = ensure_trailing_newline(
        engine::apply_hunks(&current.text.text, &hunks, line_ending)?,
        line_ending,
    );
    let content = current.text.encode(&text)?;
    let planned_file = PlannedFile {
        text: current.text.with_text(text),
        content: content.clone(),
    };

    overlay.mark_parent_directories(&destination_path);
    if remove_source {
        overlay.set(source_path.clone(), PlannedPathState::Deleted);
    }
    overlay.set(
        destination_path.clone(),
        PlannedPathState::File(planned_file),
    );

    Ok(PlannedAction::Update {
        source_path,
        destination_path: destination_path.clone(),
        content,
        summary_path: display_relative(cwd, &destination_path),
        remove_source,
    })
}

async fn ensure_writable_file_target(
    cwd: &Path,
    path: &Path,
    overlay: &PathOverlay,
) -> Result<(), PatchError> {
    if overlay.contains_directory(path) {
        return Err(PatchError::failed(format!(
            "`{}` is not a file",
            display_relative(cwd, path)
        )));
    }

    ensure_parent_directories_can_exist(cwd, path, overlay).await?;

    match overlay.get(path) {
        Some(PlannedPathState::File(_)) | Some(PlannedPathState::Deleted) => Ok(()),
        None => match fs::metadata(path).await {
            Ok(metadata) if metadata.is_file() => Ok(()),
            Ok(_) => Err(PatchError::failed(format!(
                "`{}` is not a file",
                display_relative(cwd, path)
            ))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        },
    }
}

async fn ensure_parent_directories_can_exist(
    cwd: &Path,
    path: &Path,
    overlay: &PathOverlay,
) -> Result<(), PatchError> {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent.as_os_str().is_empty() {
            break;
        }

        match overlay.get(parent) {
            Some(PlannedPathState::File(_)) => {
                return Err(PatchError::failed(format!(
                    "parent path `{}` is not a directory",
                    display_relative(cwd, parent)
                )));
            }
            Some(PlannedPathState::Deleted) => {
                current = parent.parent();
                continue;
            }
            None if overlay.contains_directory(parent) => {
                current = parent.parent();
                continue;
            }
            None => {}
        }

        match fs::metadata(parent).await {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(PatchError::failed(format!(
                    "parent path `{}` is not a directory",
                    display_relative(cwd, parent)
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        current = parent.parent();
    }

    Ok(())
}

async fn require_file(
    state: &Arc<AppState>,
    cwd: &Path,
    path: &Path,
    overlay: &PathOverlay,
) -> Result<PlannedFile, PatchError> {
    if overlay.contains_directory(path) {
        return Err(PatchError::failed(format!(
            "`{}` is not a file",
            display_relative(cwd, path)
        )));
    }

    if let Some(state) = overlay.get(path) {
        return match state {
            PlannedPathState::File(file) => Ok(file.clone()),
            PlannedPathState::Deleted => Err(PatchError::failed(format!(
                "`{}` does not exist",
                display_relative(cwd, path)
            ))),
        };
    }

    let metadata = fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(PatchError::failed(format!(
            "`{}` is not a file",
            display_relative(cwd, path)
        )));
    }
    let text = PatchTextFile::read(
        path,
        state
            .config
            .experimental_apply_patch_target_encoding_autodetect,
    )
    .await?;
    let content = text.encode(&text.text)?;
    Ok(PlannedFile { text, content })
}

async fn file_from_overlay_or_disk(
    state: &Arc<AppState>,
    cwd: &Path,
    path: &Path,
    overlay: &PathOverlay,
) -> Result<Option<PlannedFile>, PatchError> {
    if let Some(state) = overlay.get(path) {
        return match state {
            PlannedPathState::File(file) => Ok(Some(file.clone())),
            PlannedPathState::Deleted => Ok(None),
        };
    }

    match fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => {
            let text = PatchTextFile::read(
                path,
                state
                    .config
                    .experimental_apply_patch_target_encoding_autodetect,
            )
            .await?;
            let content = text.encode(&text.text)?;
            Ok(Some(PlannedFile { text, content }))
        }
        Ok(_) => Err(PatchError::failed(format!(
            "`{}` is not a file",
            display_relative(cwd, path)
        ))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn overlay_key(path: &Path) -> PathBuf {
    crate::host_path::lexical_normalize(path)
}

fn path_eq(left: &Path, right: &Path) -> bool {
    crate::path_compare::path_eq(left, right)
}
