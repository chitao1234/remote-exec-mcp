use std::path::{Component, Path, PathBuf};

use remote_exec_proto::sandbox::SandboxAccess;
use tokio::fs;

use super::ensure_trailing_newline;
use super::parser::{PatchAction, UpdateChunk};
use super::text_codec::PatchTextFile;
use crate::AppState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAction {
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
        hunks: Vec<UpdateChunk>,
        summary_path: String,
        remove_source: bool,
    },
}

pub async fn resolve_action(
    state: &std::sync::Arc<AppState>,
    cwd: &Path,
    action: PatchAction,
) -> anyhow::Result<ResolvedAction> {
    match action {
        PatchAction::Add { path, lines } => {
            let absolute_path = resolve_patch_path(cwd, &path);
            crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &absolute_path)?;
            let content = ensure_trailing_newline(lines.join("\n"), "\n");
            let content = match fs::metadata(&absolute_path).await {
                Ok(metadata) if metadata.is_file() => PatchTextFile::read(
                    &absolute_path,
                    state
                        .config
                        .experimental_apply_patch_target_encoding_autodetect,
                )
                .await?
                .encode(&content)?,
                Ok(_) => content.into_bytes(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => content.into_bytes(),
                Err(err) => return Err(err.into()),
            };
            Ok(ResolvedAction::Add {
                path: absolute_path.clone(),
                content,
                summary_path: display_relative(cwd, &absolute_path),
            })
        }
        PatchAction::Delete { path } => {
            let absolute_path = resolve_patch_path(cwd, &path);
            crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &absolute_path)?;
            let metadata = fs::metadata(&absolute_path).await?;
            anyhow::ensure!(
                metadata.is_file(),
                "`{}` is not a file",
                display_relative(cwd, &absolute_path)
            );
            let _ = PatchTextFile::read(
                &absolute_path,
                state
                    .config
                    .experimental_apply_patch_target_encoding_autodetect,
            )
            .await?;
            Ok(ResolvedAction::Delete {
                path: absolute_path.clone(),
                summary_path: display_relative(cwd, &absolute_path),
            })
        }
        PatchAction::Update {
            path,
            move_to,
            hunks,
        } => {
            let source_path = resolve_patch_path(cwd, &path);
            crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &source_path)?;
            let destination_path = move_to
                .as_ref()
                .map(|destination| resolve_patch_path(cwd, destination))
                .unwrap_or_else(|| source_path.clone());
            if destination_path != source_path {
                crate::exec::ensure_sandbox_access(state, SandboxAccess::Write, &destination_path)?;
            }
            let remove_source = move_to.is_some() && destination_path != source_path;

            Ok(ResolvedAction::Update {
                source_path,
                destination_path: destination_path.clone(),
                hunks,
                summary_path: display_relative(cwd, &destination_path),
                remove_source,
            })
        }
    }
}

fn resolve_patch_path(cwd: &Path, path: &Path) -> PathBuf {
    crate::exec::resolve_input_path(cwd, &path.as_os_str().to_string_lossy())
}

fn display_relative(base: &Path, path: &Path) -> String {
    relative_patch_path(base, path).unwrap_or_else(|| patch_display_path(path))
}

fn relative_patch_path(base: &Path, path: &Path) -> Option<String> {
    let base = lexical_normalize(base);
    let path = lexical_normalize(path);
    let base_components: Vec<_> = base.components().collect();
    let path_components: Vec<_> = path.components().collect();
    let root_len = base_components
        .iter()
        .take_while(|component| !matches!(component, Component::Normal(_)))
        .count()
        .max(
            path_components
                .iter()
                .take_while(|component| !matches!(component, Component::Normal(_)))
                .count(),
        );

    for index in 0..root_len {
        let Some(left) = base_components.get(index).copied() else {
            return None;
        };
        let Some(right) = path_components.get(index).copied() else {
            return None;
        };
        if !path_component_eq(left, right) {
            return None;
        }
    }

    let mut shared = root_len;
    while let (Some(left), Some(right)) = (
        base_components.get(shared).copied(),
        path_components.get(shared).copied(),
    ) {
        if !path_component_eq(left, right) {
            break;
        }
        shared += 1;
    }

    let mut relative = PathBuf::new();
    for component in &base_components[shared..] {
        if matches!(component, Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &path_components[shared..] {
        match component {
            Component::CurDir => {}
            Component::ParentDir => relative.push(".."),
            Component::Normal(part) => relative.push(part),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(patch_display_path(&relative))
}

fn patch_display_path(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
}

fn path_component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    let left = left.as_os_str().to_string_lossy();
    let right = right.as_os_str().to_string_lossy();

    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::display_relative;
    use std::path::Path;

    #[test]
    fn display_relative_keeps_parent_segments_when_target_is_outside_workdir() {
        #[cfg(windows)]
        let (base, path) = (
            Path::new(r"C:\workdir\blocked"),
            Path::new(r"C:\workdir\visible\demo.txt"),
        );
        #[cfg(not(windows))]
        let (base, path) = (
            Path::new("/workdir/blocked"),
            Path::new("/workdir/visible/demo.txt"),
        );

        assert_eq!(display_relative(base, path), "../visible/demo.txt");
    }
}
