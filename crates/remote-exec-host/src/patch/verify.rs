use std::path::{Component, Path, PathBuf};

use crate::AppState;

pub(super) fn resolve_patch_path(
    state: &std::sync::Arc<AppState>,
    cwd: &Path,
    path: &Path,
) -> PathBuf {
    crate::exec::resolve_input_path_with_windows_posix_root(
        cwd,
        &path.as_os_str().to_string_lossy(),
        state.config.windows_posix_root.as_deref(),
    )
}

pub(super) fn display_relative(base: &Path, path: &Path) -> String {
    relative_patch_path(base, path).unwrap_or_else(|| patch_display_path(path))
}

fn relative_patch_path(base: &Path, path: &Path) -> Option<String> {
    let base = crate::host_path::lexical_normalize(base);
    let path = crate::host_path::lexical_normalize(path);
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
        let left = base_components.get(index).copied()?;
        let right = path_components.get(index).copied()?;
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

fn path_component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    crate::path_compare::component_eq(left.as_os_str(), right.as_os_str())
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
