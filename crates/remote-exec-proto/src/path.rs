#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Posix,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathComparison {
    CaseSensitive,
    CaseInsensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPolicy {
    pub style: PathStyle,
    pub comparison: PathComparison,
}

pub fn linux_path_policy() -> PathPolicy {
    PathPolicy {
        style: PathStyle::Posix,
        comparison: PathComparison::CaseSensitive,
    }
}

pub fn windows_path_policy() -> PathPolicy {
    PathPolicy {
        style: PathStyle::Windows,
        comparison: PathComparison::CaseInsensitive,
    }
}

fn split_windows_prefix(raw: &str) -> (&str, &str) {
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return (&raw[..2], &raw[2..]);
    }
    if raw.starts_with(r"\\") || raw.starts_with("//") {
        return (&raw[..2], &raw[2..]);
    }
    ("", raw)
}

fn normalize_windows_separators(raw: &str) -> String {
    let (prefix, rest) = split_windows_prefix(raw);
    let normalized_rest = rest
        .chars()
        .map(|ch| match ch {
            '/' | '\\' => '\\',
            other => other,
        })
        .collect::<String>();
    format!("{prefix}{normalized_rest}")
}

fn comparison_key(policy: PathPolicy, raw: &str) -> String {
    let normalized = match policy.style {
        PathStyle::Posix => raw.to_string(),
        PathStyle::Windows => normalize_windows_separators(raw),
    };

    match policy.comparison {
        PathComparison::CaseSensitive => normalized,
        PathComparison::CaseInsensitive => normalized.to_ascii_lowercase(),
    }
}

pub fn is_absolute_for_policy(policy: PathPolicy, raw: &str) -> bool {
    match policy.style {
        PathStyle::Posix => raw.starts_with('/'),
        PathStyle::Windows => {
            let bytes = raw.as_bytes();
            (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/'))
                || raw.starts_with(r"\\")
                || raw.starts_with("//")
        }
    }
}

pub fn normalize_for_system(policy: PathPolicy, raw: &str) -> String {
    match policy.style {
        PathStyle::Posix => raw.to_string(),
        PathStyle::Windows => normalize_windows_separators(raw),
    }
}

pub fn same_path_for_policy(policy: PathPolicy, left: &str, right: &str) -> bool {
    comparison_key(policy, left) == comparison_key(policy, right)
}

#[cfg(test)]
mod tests {
    use super::{
        is_absolute_for_policy, linux_path_policy, normalize_for_system, same_path_for_policy,
        windows_path_policy,
    };

    #[test]
    fn windows_absolute_path_accepts_both_separator_forms() {
        let policy = windows_path_policy();
        assert!(is_absolute_for_policy(policy, r"C:\work\artifact.txt"));
        assert!(is_absolute_for_policy(policy, "C:/work/artifact.txt"));
        assert!(!is_absolute_for_policy(policy, r"work\artifact.txt"));
    }

    #[test]
    fn windows_same_path_ignores_case_and_separator_style() {
        let policy = windows_path_policy();
        assert!(same_path_for_policy(
            policy,
            r"C:\Work\Artifact.txt",
            "c:/work/artifact.txt"
        ));
    }

    #[test]
    fn linux_same_path_preserves_case_sensitivity() {
        let policy = linux_path_policy();
        assert!(!same_path_for_policy(
            policy,
            "/tmp/Artifact.txt",
            "/tmp/artifact.txt"
        ));
    }

    #[test]
    fn windows_system_normalization_emits_backslashes() {
        let policy = windows_path_policy();
        assert_eq!(
            normalize_for_system(policy, "C:/work/releases/current.txt"),
            r"C:\work\releases\current.txt"
        );
    }
}
