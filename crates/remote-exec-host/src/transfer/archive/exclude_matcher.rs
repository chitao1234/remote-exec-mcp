use std::path::{Component, Path};

use anyhow::Context;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

#[derive(Debug)]
pub(super) struct ExcludeMatcher {
    set: GlobSet,
}

impl ExcludeMatcher {
    pub(super) fn compile(patterns: &[String]) -> anyhow::Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .backslash_escape(true)
                .build()
                .with_context(|| format!("invalid exclude pattern `{pattern}`"))?;
            builder.add(glob);
        }

        Ok(Self {
            set: builder.build().context("building exclude matcher")?,
        })
    }

    pub(super) fn is_excluded_path(&self, relative_path: &str) -> bool {
        !relative_path.is_empty() && self.set.is_match(relative_path)
    }

    pub(super) fn is_excluded_directory(&self, relative_path: &str) -> bool {
        if self.is_excluded_path(relative_path) {
            return true;
        }
        if relative_path.is_empty() {
            return false;
        }

        let mut with_separator = String::with_capacity(relative_path.len() + 1);
        with_separator.push_str(relative_path);
        with_separator.push('/');
        self.set.is_match(with_separator)
    }
}

pub(super) fn normalize_relative_path(path: &Path) -> String {
    let mut normalized = String::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(&part.to_string_lossy());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str("..");
            }
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::ExcludeMatcher;

    fn patterns(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn matches_relative_paths_with_supported_character_classes() {
        let matcher = ExcludeMatcher::compile(&patterns(&[
            "**/*.log",
            "src/[ab].rs",
            "docs/[a-z].md",
            "tmp/[!abc].txt",
            "tmp/[^a-c].cfg",
            "lib/[!a-c].hpp",
            "lib/[^abc].ipp",
        ]))
        .expect("compile matcher");

        assert!(matcher.is_excluded_path("build/app.log"));
        assert!(matcher.is_excluded_path("src/a.rs"));
        assert!(!matcher.is_excluded_path("src/z.rs"));
        assert!(matcher.is_excluded_path("docs/m.md"));
        assert!(!matcher.is_excluded_path("docs/MM.md"));
        assert!(matcher.is_excluded_path("tmp/z.txt"));
        assert!(!matcher.is_excluded_path("tmp/a.txt"));
        assert!(matcher.is_excluded_path("tmp/z.cfg"));
        assert!(!matcher.is_excluded_path("tmp/b.cfg"));
        assert!(matcher.is_excluded_path("lib/z.hpp"));
        assert!(!matcher.is_excluded_path("lib/b.hpp"));
        assert!(matcher.is_excluded_path("lib/z.ipp"));
        assert!(!matcher.is_excluded_path("lib/c.ipp"));
    }

    #[test]
    fn directory_checks_match_subtree_patterns() {
        let matcher =
            ExcludeMatcher::compile(&patterns(&[".git/**", "cache"])).expect("compile matcher");

        assert!(matcher.is_excluded_directory(".git"));
        assert!(matcher.is_excluded_directory("cache"));
        assert!(!matcher.is_excluded_directory("src"));
    }

    #[test]
    fn rejects_malformed_globs() {
        let err = ExcludeMatcher::compile(&patterns(&["tmp/[abc"])).expect_err("invalid glob");
        assert!(
            err.to_string().contains("exclude pattern"),
            "unexpected error: {err}"
        );
    }
}
