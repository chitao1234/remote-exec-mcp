use std::path::{Path, PathBuf};

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
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}
