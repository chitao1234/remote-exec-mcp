use std::path::{Path, PathBuf};

use tokio::fs;

use super::engine;
use super::parser::PatchAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedAction {
    Add {
        path: PathBuf,
        content: String,
        summary_path: String,
    },
    Delete {
        path: PathBuf,
        summary_path: String,
    },
    Update {
        source_path: PathBuf,
        destination_path: PathBuf,
        content: String,
        summary_path: String,
        remove_source: bool,
    },
}

pub async fn verify_actions(
    cwd: &Path,
    actions: Vec<PatchAction>,
) -> anyhow::Result<Vec<VerifiedAction>> {
    let mut verified = Vec::with_capacity(actions.len());

    for action in actions {
        match action {
            PatchAction::Add { path, lines } => {
                let absolute_path = cwd.join(&path);
                verified.push(VerifiedAction::Add {
                    path: absolute_path.clone(),
                    content: ensure_trailing_newline(lines.join("\n")),
                    summary_path: display_relative(cwd, &absolute_path),
                });
            }
            PatchAction::Delete { path } => {
                let absolute_path = cwd.join(&path);
                let metadata = fs::metadata(&absolute_path).await?;
                anyhow::ensure!(
                    metadata.is_file(),
                    "`{}` is not a file",
                    display_relative(cwd, &absolute_path)
                );
                let _ = fs::read_to_string(&absolute_path).await?;
                verified.push(VerifiedAction::Delete {
                    path: absolute_path.clone(),
                    summary_path: display_relative(cwd, &absolute_path),
                });
            }
            PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let source_path = cwd.join(&path);
                let current = fs::read_to_string(&source_path).await?;
                let destination_path = move_to
                    .as_ref()
                    .map(|destination| cwd.join(destination))
                    .unwrap_or_else(|| source_path.clone());
                let remove_source = move_to.is_some() && destination_path != source_path;
                let content = ensure_trailing_newline(engine::apply_hunks(&current, &hunks)?);

                verified.push(VerifiedAction::Update {
                    source_path,
                    destination_path: destination_path.clone(),
                    content,
                    summary_path: display_relative(cwd, &destination_path),
                    remove_source,
                });
            }
        }
    }

    Ok(verified)
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn display_relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}
