#[cfg(not(windows))]
use std::process::Command;
#[cfg(not(windows))]
use std::sync::OnceLock;

use crate::config::ProcessEnvironment;

#[cfg(not(windows))]
const TEST_LOCALE_OUTPUT_ENV: &str = "REMOTE_EXEC_TEST_LOCALE_OUTPUT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocaleStrategy {
    #[cfg(not(windows))]
    Direct(String),
    #[cfg(not(windows))]
    HybridCType(String),
    #[cfg(not(windows))]
    #[allow(dead_code)]
    LastResortLcAll(String),
    LangCOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocaleEnvPlan {
    strategy: LocaleStrategy,
}

impl LocaleEnvPlan {
    pub(crate) fn from_strategy(strategy: LocaleStrategy) -> Self {
        Self { strategy }
    }

    pub(crate) fn resolved(_environment: &ProcessEnvironment) -> Self {
        #[cfg(windows)]
        {
            LocaleEnvPlan::from_strategy(LocaleStrategy::LangCOnly)
        }

        #[cfg(not(windows))]
        {
            if let Some(plan) = resolved_from_override_env(_environment) {
                return plan;
            }

            static CACHE: OnceLock<LocaleEnvPlan> = OnceLock::new();
            CACHE.get_or_init(resolve_locale_env_plan).clone()
        }
    }

    pub(crate) fn as_pairs(&self) -> Vec<(String, String)> {
        #[cfg(windows)]
        {
            Vec::new()
        }

        #[cfg(not(windows))]
        {
            match &self.strategy {
                LocaleStrategy::Direct(locale) => vec![
                    ("LANG".to_string(), locale.clone()),
                    ("LC_CTYPE".to_string(), locale.clone()),
                    ("LC_ALL".to_string(), locale.clone()),
                ],
                LocaleStrategy::HybridCType(locale) => vec![
                    ("LANG".to_string(), "C".to_string()),
                    ("LC_CTYPE".to_string(), locale.clone()),
                ],
                LocaleStrategy::LastResortLcAll(locale) => vec![
                    ("LANG".to_string(), "C".to_string()),
                    ("LC_ALL".to_string(), locale.clone()),
                ],
                LocaleStrategy::LangCOnly => vec![("LANG".to_string(), "C".to_string())],
            }
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn choose_strategy<I, S>(locales: I) -> LocaleStrategy
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let locales = locales
        .into_iter()
        .map(|locale| locale.as_ref().trim().to_string())
        .filter(|locale| !locale.is_empty())
        .collect::<Vec<_>>();

    if locales.iter().any(|locale| locale == "C.UTF-8") {
        return LocaleStrategy::Direct("C.UTF-8".to_string());
    }

    if locales.iter().any(|locale| locale == "C.utf8") {
        return LocaleStrategy::Direct("C.utf8".to_string());
    }

    if let Some(locale) = best_utf8_locale(&locales) {
        return LocaleStrategy::HybridCType(locale);
    }

    LocaleStrategy::LangCOnly
}

#[cfg(not(windows))]
fn resolved_from_override_env(environment: &ProcessEnvironment) -> Option<LocaleEnvPlan> {
    let output = environment.var_os(TEST_LOCALE_OUTPUT_ENV)?;
    let output = output.to_string_lossy();
    Some(LocaleEnvPlan::from_strategy(choose_strategy(
        parse_locale_output(&output),
    )))
}

#[cfg(not(windows))]
fn resolve_locale_env_plan() -> LocaleEnvPlan {
    let locales = discover_locales().unwrap_or_default();
    LocaleEnvPlan::from_strategy(choose_strategy(locales))
}

#[cfg(not(windows))]
fn discover_locales() -> Option<Vec<String>> {
    let output = locale_command_output()?;
    Some(parse_locale_output(&output))
}

#[cfg(not(windows))]
fn locale_command_output() -> Option<String> {
    let output = Command::new("locale").arg("-a").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(not(windows))]
fn parse_locale_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(not(windows))]
fn best_utf8_locale(locales: &[String]) -> Option<String> {
    let mut candidates = locales
        .iter()
        .filter(|locale| is_utf8_locale(locale))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|locale| locale_rank(locale));
    candidates.into_iter().next()
}

#[cfg(not(windows))]
fn is_utf8_locale(locale: &str) -> bool {
    let lower = locale.to_ascii_lowercase();
    lower.ends_with(".utf-8") || lower.ends_with(".utf8")
}

#[cfg(not(windows))]
fn locale_rank(locale: &str) -> (u8, String) {
    let lower = locale.to_ascii_lowercase();
    if lower == "en_us.utf-8" {
        return (0, lower);
    }
    if lower == "en_us.utf8" {
        return (1, lower);
    }
    if lower == "en_gb.utf-8" {
        return (2, lower);
    }
    if lower == "en_gb.utf8" {
        return (3, lower);
    }
    if lower.starts_with("en_") && (lower.ends_with(".utf-8") || lower.ends_with(".utf8")) {
        return (4, lower);
    }
    (5, lower)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::{LocaleEnvPlan, LocaleStrategy};
    #[cfg(not(windows))]
    use super::{LocaleEnvPlan, LocaleStrategy, choose_strategy};

    #[cfg(not(windows))]
    #[test]
    fn prefers_c_utf8_over_other_utf8_locales() {
        let strategy = choose_strategy(["en_US.UTF-8", "C.UTF-8", "fr_FR.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::Direct("C.UTF-8".to_string()));
    }

    #[cfg(not(windows))]
    #[test]
    fn accepts_c_utf8_lowercase_variant_when_exact_name_is_absent() {
        let strategy = choose_strategy(["C.utf8", "en_US.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::Direct("C.utf8".to_string()));
    }

    #[cfg(not(windows))]
    #[test]
    fn prefers_english_utf8_locale_for_hybrid_fallback() {
        let strategy = choose_strategy(["fr_FR.UTF-8", "en_US.UTF-8"]);
        assert_eq!(
            strategy,
            LocaleStrategy::HybridCType("en_US.UTF-8".to_string())
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn prefers_english_family_before_other_utf8_locales() {
        let strategy = choose_strategy(["fr_FR.UTF-8", "en_AU.UTF-8"]);
        assert_eq!(
            strategy,
            LocaleStrategy::HybridCType("en_AU.UTF-8".to_string())
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn falls_back_to_non_english_utf8_when_no_english_choice_exists() {
        let strategy = choose_strategy(["fr_FR.UTF-8"]);
        assert_eq!(
            strategy,
            LocaleStrategy::HybridCType("fr_FR.UTF-8".to_string())
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn falls_back_to_lang_c_only_when_no_utf8_locale_exists() {
        let strategy = choose_strategy(["C", "POSIX", "en_US.ISO8859-1"]);
        assert_eq!(strategy, LocaleStrategy::LangCOnly);
    }

    #[cfg(unix)]
    #[test]
    fn locale_env_plan_matches_direct_strategy_shape() {
        let plan = LocaleEnvPlan::from_strategy(LocaleStrategy::Direct("C.UTF-8".to_string()));
        assert_eq!(
            plan.as_pairs(),
            vec![
                ("LANG".to_string(), "C.UTF-8".to_string()),
                ("LC_CTYPE".to_string(), "C.UTF-8".to_string()),
                ("LC_ALL".to_string(), "C.UTF-8".to_string()),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_env_plan_matches_hybrid_strategy_shape() {
        let plan =
            LocaleEnvPlan::from_strategy(LocaleStrategy::HybridCType("en_US.UTF-8".to_string()));
        assert_eq!(
            plan.as_pairs(),
            vec![
                ("LANG".to_string(), "C".to_string()),
                ("LC_CTYPE".to_string(), "en_US.UTF-8".to_string()),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_env_plan_matches_lang_c_only_shape() {
        let plan = LocaleEnvPlan::from_strategy(LocaleStrategy::LangCOnly);
        assert_eq!(plan.as_pairs(), vec![("LANG".to_string(), "C".to_string())]);
    }

    #[cfg(windows)]
    #[test]
    fn locale_env_plan_is_empty_on_windows() {
        let plan = LocaleEnvPlan::from_strategy(LocaleStrategy::LangCOnly);
        assert!(plan.as_pairs().is_empty());
    }
}
