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
            tail.chars()
                .map(|ch| match ch {
                    '/' | '\\' => '\\',
                    other => other,
                })
                .collect::<String>()
        )
    }
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
        PathStyle::Windows => normalize_for_system(policy, raw),
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
                || translate_windows_posix_drive_path(raw).is_some()
        }
    }
}

pub fn normalize_for_system(policy: PathPolicy, raw: &str) -> String {
    match policy.style {
        PathStyle::Posix => raw.to_string(),
        PathStyle::Windows => translate_windows_posix_drive_path(raw)
            .unwrap_or_else(|| normalize_windows_separators(raw)),
    }
}

pub fn join_for_policy(policy: PathPolicy, base: &str, child: &str) -> String {
    let normalized_child = normalize_for_system(policy, child);
    if normalized_child.is_empty() || is_absolute_for_policy(policy, &normalized_child) {
        return normalized_child;
    }

    let normalized_base = normalize_for_system(policy, base);
    if normalized_base.is_empty() {
        return normalized_child;
    }

    let separator = match policy.style {
        PathStyle::Posix => '/',
        PathStyle::Windows => '\\',
    };

    if normalized_base.ends_with(separator) {
        format!("{normalized_base}{normalized_child}")
    } else {
        format!("{normalized_base}{separator}{normalized_child}")
    }
}

pub fn same_path_for_policy(policy: PathPolicy, left: &str, right: &str) -> bool {
    comparison_key(policy, left) == comparison_key(policy, right)
}

#[cfg(test)]
mod tests {
    use super::{
        is_absolute_for_policy, join_for_policy, linux_path_policy, normalize_for_system,
        same_path_for_policy, windows_path_policy,
    };

    #[test]
    fn windows_absolute_path_accepts_both_separator_forms() {
        let policy = windows_path_policy();
        assert!(is_absolute_for_policy(policy, r"C:\work\artifact.txt"));
        assert!(is_absolute_for_policy(policy, "C:/work/artifact.txt"));
        assert!(is_absolute_for_policy(policy, "/c/work/artifact.txt"));
        assert!(is_absolute_for_policy(
            policy,
            "/cygdrive/c/work/artifact.txt"
        ));
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
        assert!(same_path_for_policy(
            policy,
            "/c/Work/Artifact.txt",
            r"c:\work\artifact.txt"
        ));
        assert!(same_path_for_policy(
            policy,
            "/cygdrive/c/Work/Artifact.txt",
            r"c:\work\artifact.txt"
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
        assert_eq!(
            normalize_for_system(policy, "/c/work/releases/current.txt"),
            r"C:\work\releases\current.txt"
        );
        assert_eq!(
            normalize_for_system(policy, "/cygdrive/c/work/releases/current.txt"),
            r"C:\work\releases\current.txt"
        );
    }

    #[test]
    fn linux_join_uses_forward_slashes() {
        let policy = linux_path_policy();
        assert_eq!(
            join_for_policy(policy, "outer", "nested/file.txt"),
            "outer/nested/file.txt"
        );
    }

    #[test]
    fn windows_join_normalizes_both_separator_styles() {
        let policy = windows_path_policy();
        assert_eq!(
            join_for_policy(policy, "C:/work/releases", "nested/file.txt"),
            r"C:\work\releases\nested\file.txt"
        );
        assert_eq!(
            join_for_policy(policy, r"C:\work\releases", r"nested\file.txt"),
            r"C:\work\releases\nested\file.txt"
        );
        assert_eq!(
            join_for_policy(policy, r"C:\work\releases", "/c/other/file.txt"),
            r"C:\other\file.txt"
        );
    }
}
