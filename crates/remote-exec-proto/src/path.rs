use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Posix,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPolicy {
    pub style: PathStyle,
}

impl PathPolicy {
    pub fn is_absolute(self, raw: &str) -> bool {
        match self.style {
            PathStyle::Posix => raw.starts_with('/'),
            PathStyle::Windows => {
                let bytes = raw.as_bytes();
                (bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && (bytes[2] == b'\\' || bytes[2] == b'/'))
                    || raw.starts_with(r"\\")
                    || raw.starts_with("//")
                    || translate_windows_posix_drive_path(raw).is_some()
            }
        }
    }

    pub fn normalize_for_system(self, raw: &str) -> String {
        match self.style {
            PathStyle::Posix => raw.to_string(),
            PathStyle::Windows => translate_windows_posix_drive_path(raw)
                .unwrap_or_else(|| normalize_windows_separators(raw)),
        }
    }

    pub fn syntax_eq(self, left: &str, right: &str) -> bool {
        self.normalize_for_system(left) == self.normalize_for_system(right)
    }

    pub fn basename(self, raw: &str) -> Option<String> {
        let normalized = self.normalize_for_system(raw);
        match self.style {
            PathStyle::Posix => normalized
                .trim_end_matches('/')
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .map(str::to_string),
            PathStyle::Windows => split_windows_path_basename(&normalized),
        }
    }

    pub fn join(self, base: &str, child: &str) -> String {
        let normalized_child = self.normalize_for_system(child);
        if normalized_child.is_empty() || self.is_absolute(&normalized_child) {
            return normalized_child;
        }

        let normalized_base = self.normalize_for_system(base);
        if normalized_base.is_empty() {
            return normalized_child;
        }

        let separator = match self.style {
            PathStyle::Posix => '/',
            PathStyle::Windows => '\\',
        };

        if normalized_base.ends_with(separator) {
            format!("{normalized_base}{normalized_child}")
        } else {
            format!("{normalized_base}{separator}{normalized_child}")
        }
    }
}

pub fn linux_path_policy() -> PathPolicy {
    PathPolicy {
        style: PathStyle::Posix,
    }
}

pub fn windows_path_policy() -> PathPolicy {
    PathPolicy {
        style: PathStyle::Windows,
    }
}

pub fn host_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn split_windows_prefix(raw: &str) -> (&str, &str) {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return (&raw[..2], &raw[2..]);
    }
    if raw.starts_with(r"\\") || raw.starts_with("//") {
        return (&raw[..2], &raw[2..]);
    }
    ("", raw)
}

fn translate_windows_posix_drive_path(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && (bytes.len() == 2 || bytes[2] == b'/')
    {
        return Some(build_windows_drive_path(
            bytes[1] as char,
            raw.get(2..).unwrap_or_default(),
        ));
    }

    let lower = raw.to_ascii_lowercase();
    let rest = lower.strip_prefix("/cygdrive/")?;
    let rest_bytes = rest.as_bytes();
    if rest_bytes.is_empty()
        || !rest_bytes[0].is_ascii_alphabetic()
        || (rest_bytes.len() > 1 && rest_bytes[1] != b'/')
    {
        return None;
    }

    Some(build_windows_drive_path(
        raw.as_bytes()["/cygdrive/".len()] as char,
        raw.get("/cygdrive/".len() + 1..).unwrap_or_default(),
    ))
}

fn build_windows_drive_path(drive: char, rest: &str) -> String {
    let tail = rest.trim_start_matches(['/', '\\']);
    if tail.is_empty() {
        format!("{}:\\", drive.to_ascii_uppercase())
    } else {
        format!(
            "{}:\\{}",
            drive.to_ascii_uppercase(),
            normalize_windows_path_chars(tail)
        )
    }
}

fn normalize_windows_path_chars(raw: &str) -> String {
    raw.chars()
        .map(|ch| match ch {
            '/' | '\\' => '\\',
            other => other,
        })
        .collect()
}

fn normalize_windows_separators(raw: &str) -> String {
    let (prefix, rest) = split_windows_prefix(raw);
    let normalized_prefix = normalize_windows_path_chars(prefix);
    let normalized_rest = normalize_windows_path_chars(rest);
    format!("{normalized_prefix}{normalized_rest}")
}

fn split_windows_path_basename(raw: &str) -> Option<String> {
    let (prefix, rest) = split_windows_prefix(raw);
    let trimmed = rest.trim_end_matches('\\');
    if trimmed.is_empty() {
        return (!prefix.is_empty())
            .then(|| {
                prefix
                    .trim_end_matches([':', '\\'])
                    .trim_start_matches('\\')
                    .to_string()
            })
            .filter(|segment| !segment.is_empty());
    }

    trimmed
        .rsplit('\\')
        .find(|segment| !segment.is_empty())
        .map(str::to_string)
}

pub fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::{linux_path_policy, normalize_relative_path, windows_path_policy};

    #[test]
    fn windows_absolute_path_accepts_both_separator_forms() {
        let policy = windows_path_policy();
        assert!(policy.is_absolute(r"C:\work\artifact.txt"));
        assert!(policy.is_absolute("C:/work/artifact.txt"));
        assert!(policy.is_absolute("/c/work/artifact.txt"));
        assert!(policy.is_absolute("/cygdrive/c/work/artifact.txt"));
        assert!(!policy.is_absolute(r"work\artifact.txt"));
    }

    #[test]
    fn windows_syntax_eq_normalizes_separator_style_and_drive_aliases() {
        let policy = windows_path_policy();
        assert!(policy.syntax_eq(r"C:\Work\Artifact.txt", "C:/Work/Artifact.txt"));
        assert!(policy.syntax_eq("/c/Work/Artifact.txt", r"C:\Work\Artifact.txt"));
        assert!(policy.syntax_eq("/cygdrive/c/Work/Artifact.txt", r"C:\Work\Artifact.txt"));
    }

    #[test]
    fn windows_syntax_eq_preserves_case_differences() {
        let policy = windows_path_policy();
        assert!(!policy.syntax_eq(r"C:\RÉSUMÉ\Ärger.txt", "c:/résumé/ärger.TXT"));
    }

    #[test]
    fn linux_syntax_eq_preserves_case_sensitivity() {
        let policy = linux_path_policy();
        assert!(!policy.syntax_eq("/tmp/Artifact.txt", "/tmp/artifact.txt"));
    }

    #[test]
    fn windows_system_normalization_emits_backslashes() {
        let policy = windows_path_policy();
        assert_eq!(
            policy.normalize_for_system("C:/work/releases/current.txt"),
            r"C:\work\releases\current.txt"
        );
        assert_eq!(
            policy.normalize_for_system("/c/work/releases/current.txt"),
            r"C:\work\releases\current.txt"
        );
        assert_eq!(
            policy.normalize_for_system("/cygdrive/c/work/releases/current.txt"),
            r"C:\work\releases\current.txt"
        );
        assert_eq!(
            policy.normalize_for_system("//server/share/releases/current.txt"),
            r"\\server\share\releases\current.txt"
        );
    }

    #[test]
    fn linux_join_uses_forward_slashes() {
        let policy = linux_path_policy();
        assert_eq!(
            policy.join("outer", "nested/file.txt"),
            "outer/nested/file.txt"
        );
    }

    #[test]
    fn basename_for_policy_handles_posix_paths() {
        let policy = linux_path_policy();
        assert_eq!(
            policy.basename("/tmp/build/output.tar"),
            Some("output.tar".to_string())
        );
        assert_eq!(policy.basename("/"), None);
    }

    #[test]
    fn basename_for_policy_handles_windows_paths() {
        let policy = windows_path_policy();
        assert_eq!(
            policy.basename("C:/work/releases/current.txt"),
            Some("current.txt".to_string())
        );
        assert_eq!(
            policy.basename("/cygdrive/c/work/releases"),
            Some("releases".to_string())
        );
        assert_eq!(policy.basename(r"C:\"), Some("C".to_string()));
    }

    #[test]
    fn windows_join_normalizes_both_separator_styles() {
        let policy = windows_path_policy();
        assert_eq!(
            policy.join("C:/work/releases", "nested/file.txt"),
            r"C:\work\releases\nested\file.txt"
        );
        assert_eq!(
            policy.join(r"C:\work\releases", r"nested\file.txt"),
            r"C:\work\releases\nested\file.txt"
        );
        assert_eq!(
            policy.join(r"C:\work\releases", "/c/other/file.txt"),
            r"C:\other\file.txt"
        );
    }

    #[test]
    fn normalize_relative_path_rejects_parent_traversal() {
        assert!(normalize_relative_path(std::path::Path::new("../escape.txt")).is_none());
        assert!(normalize_relative_path(std::path::Path::new("nested/../../escape.txt")).is_none());
    }

    #[test]
    fn normalize_relative_path_collapses_current_dir_components() {
        assert_eq!(
            normalize_relative_path(std::path::Path::new("./nested/./hello.txt")).unwrap(),
            std::path::PathBuf::from("nested").join("hello.txt")
        );
    }
}
