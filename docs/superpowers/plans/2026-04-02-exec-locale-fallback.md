# Exec Locale Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the daemon's fixed `C.UTF-8` child locale overlay with a cached host-supported fallback strategy that preserves C-style behavior and UTF-8 whenever possible.

**Architecture:** Add a focused locale helper under the daemon exec runtime that discovers supported locales via `locale -a`, ranks them according to the approved policy, caches the chosen strategy, and exposes the final locale env overlay for both PTY and pipe-backed spawns. Keep the non-locale env overlay unchanged, and add deterministic tests through a process-env override seam for synthetic discovered locales; when that override is present, the helper must bypass the normal cache so tests stay isolated.

**Tech Stack:** Rust 2024, Tokio, daemon RPC integration tests, unit tests, `cargo test`

---

## File Map

- Create: `crates/remote-exec-daemon/src/exec/locale.rs`
  - Own locale discovery, ranking, cached strategy resolution, and the env-var-based test seam.
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
  - Register the new helper module.
- Modify: `crates/remote-exec-daemon/src/exec/session.rs`
  - Replace the fixed locale entries in the env overlay with values produced by the locale helper.
- Modify: `crates/remote-exec-daemon/tests/exec_rpc.rs`
  - Update env-overlay assertions and add deterministic locale env coverage for pipe and PTY mode using the env-var seam.

### Task 1: Add Locale Strategy Discovery And Ranking

**Files:**
- Create: `crates/remote-exec-daemon/src/exec/locale.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Test/Verify: `cargo test -p remote-exec-daemon locale:: --lib -- --nocapture`

**Testing approach:** `TDD`
Reason: the locale ranking and fallback rules are pure, externally meaningful behavior with a clean unit-test seam. TDD fits well here and avoids hand-wavy fallback logic.

- [ ] **Step 1: Add failing unit tests for locale strategy selection in the new helper module**

```rust
#[cfg(test)]
mod tests {
    use super::{LocaleEnvPlan, LocaleStrategy, choose_strategy};

    #[test]
    fn prefers_c_utf8_over_other_utf8_locales() {
        let strategy = choose_strategy(["en_US.UTF-8", "C.UTF-8", "fr_FR.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::Direct("C.UTF-8".to_string()));
    }

    #[test]
    fn accepts_c_utf8_lowercase_variant_when_exact_name_is_absent() {
        let strategy = choose_strategy(["C.utf8", "en_US.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::Direct("C.utf8".to_string()));
    }

    #[test]
    fn prefers_english_utf8_locale_for_hybrid_fallback() {
        let strategy = choose_strategy(["fr_FR.UTF-8", "en_US.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::HybridCType("en_US.UTF-8".to_string()));
    }

    #[test]
    fn prefers_english_family_before_other_utf8_locales() {
        let strategy = choose_strategy(["fr_FR.UTF-8", "en_AU.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::HybridCType("en_AU.UTF-8".to_string()));
    }

    #[test]
    fn falls_back_to_non_english_utf8_when_no_english_choice_exists() {
        let strategy = choose_strategy(["fr_FR.UTF-8"]);
        assert_eq!(strategy, LocaleStrategy::HybridCType("fr_FR.UTF-8".to_string()));
    }

    #[test]
    fn falls_back_to_lang_c_only_when_no_utf8_locale_exists() {
        let strategy = choose_strategy(["C", "POSIX", "en_US.ISO8859-1"]);
        assert_eq!(strategy, LocaleStrategy::LangCOnly);
    }

    #[test]
    fn locale_env_plan_matches_direct_strategy_shape() {
        let plan = LocaleEnvPlan::from_strategy(LocaleStrategy::Direct("C.UTF-8".to_string()));
        assert_eq!(plan.as_pairs(), vec![
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("LC_CTYPE".to_string(), "C.UTF-8".to_string()),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ]);
    }

    #[test]
    fn locale_env_plan_matches_hybrid_strategy_shape() {
        let plan =
            LocaleEnvPlan::from_strategy(LocaleStrategy::HybridCType("en_US.UTF-8".to_string()));
        assert_eq!(plan.as_pairs(), vec![
            ("LANG".to_string(), "C".to_string()),
            ("LC_CTYPE".to_string(), "en_US.UTF-8".to_string()),
        ]);
    }

    #[test]
    fn locale_env_plan_matches_lang_c_only_shape() {
        let plan = LocaleEnvPlan::from_strategy(LocaleStrategy::LangCOnly);
        assert_eq!(plan.as_pairs(), vec![("LANG".to_string(), "C".to_string())]);
    }
}
```

- [ ] **Step 2: Run the new locale helper tests and confirm they fail before implementation**

```bash
cargo test -p remote-exec-daemon locale:: --lib -- --nocapture
```

Expected: FAIL because `crates/remote-exec-daemon/src/exec/locale.rs` and its test-covered types/functions do not exist yet.

- [ ] **Step 3: Implement the locale helper module and wire it into `exec/mod.rs`**

```rust
// crates/remote-exec-daemon/src/exec/mod.rs
mod locale;
mod output;
pub mod session;
mod shell;
pub mod store;
pub mod transcript;
```

```rust
// crates/remote-exec-daemon/src/exec/locale.rs
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocaleStrategy {
    Direct(String),
    HybridCType(String),
    LastResortLcAll(String),
    LangCOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleEnvPlan {
    strategy: LocaleStrategy,
}

impl LocaleEnvPlan {
    pub fn from_strategy(strategy: LocaleStrategy) -> Self {
        Self { strategy }
    }

    pub fn resolved() -> Self {
        if let Some(plan) = resolved_from_override_env() {
            return plan;
        }

        static CACHE: OnceLock<LocaleEnvPlan> = OnceLock::new();
        CACHE.get_or_init(resolve_locale_env_plan).clone()
    }

    pub fn as_pairs(&self) -> Vec<(String, String)> {
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

pub fn choose_strategy<I, S>(locales: I) -> LocaleStrategy
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

fn best_utf8_locale(locales: &[String]) -> Option<String> {
    let mut candidates = locales
        .iter()
        .filter(|locale| is_utf8_locale(locale))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|locale| locale_rank(locale));
    candidates.into_iter().next()
}

fn is_utf8_locale(locale: &str) -> bool {
    let lower = locale.to_ascii_lowercase();
    lower.ends_with(".utf-8") || lower.ends_with(".utf8")
}

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

fn resolve_locale_env_plan() -> LocaleEnvPlan {
    let locales = discover_locales().unwrap_or_default();
    LocaleEnvPlan::from_strategy(choose_strategy(locales))
}

fn discover_locales() -> Option<Vec<String>> {
    let output = locale_command_output()?;
    Some(
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn locale_command_output() -> Option<String> {
    if let Ok(output) = std::env::var("REMOTE_EXEC_TEST_LOCALE_OUTPUT") {
        return Some(output);
    }

    let output = Command::new("locale").arg("-a").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn resolved_from_override_env() -> Option<LocaleEnvPlan> {
    let output = std::env::var("REMOTE_EXEC_TEST_LOCALE_OUTPUT").ok()?;
    Some(LocaleEnvPlan::from_strategy(choose_strategy(
        output.lines().map(str::trim).filter(|line| !line.is_empty()),
    )))
}
```

- [ ] **Step 4: Re-run the locale helper tests**

```bash
cargo test -p remote-exec-daemon locale:: --lib -- --nocapture
```

Expected: PASS for the new unit-style locale selection tests.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/exec/mod.rs crates/remote-exec-daemon/src/exec/locale.rs
git commit -m "feat: add exec locale fallback selection"
```

### Task 2: Apply The Locale Plan To Child Process Environment

**Files:**
- Modify: `crates/remote-exec-daemon/src/exec/session.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc env_overlay_is_applied_in_pipe_mode -- --exact --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc env_overlay_is_applied_in_pty_mode -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: this task changes observable child-process environment behavior through the daemon's spawn path. Focused daemon RPC tests are the right seam.

- [ ] **Step 1: Tighten the existing daemon env-overlay tests to include locale-related output**

```rust
#[tokio::test]
async fn env_overlay_is_applied_in_pipe_mode() {
    let _env = EnvOverrideGuard::set(&[
        ("TERM", "rainbow-terminal"),
        ("NO_COLOR", "0"),
        ("PAGER", "less"),
        ("GIT_PAGER", "more"),
        ("CODEX_CI", "0"),
        ("LANG", "fr_FR.UTF-8"),
        ("LC_CTYPE", "fr_FR.UTF-8"),
        ("LC_ALL", "fr_FR.UTF-8"),
    ])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s|%s|%s|%s|%s|%s' \"$TERM\" \"$NO_COLOR\" \"$PAGER\" \"$GIT_PAGER\" \"$CODEX_CI\" \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\""
                    .to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "dumb|1|cat|cat|1|C.UTF-8|C.UTF-8|C.UTF-8");
}
```

Repeat the same command/assertion shape for the PTY-mode test.

- [ ] **Step 2: Run the focused env-overlay tests and confirm they fail before the session code is updated**

```bash
cargo test -p remote-exec-daemon --test exec_rpc env_overlay_is_applied_in_pipe_mode -- --exact --nocapture
cargo test -p remote-exec-daemon --test exec_rpc env_overlay_is_applied_in_pty_mode -- --exact --nocapture
```

Expected: FAIL once the assertions are tightened but before `session.rs` consumes the new locale helper.

- [ ] **Step 3: Replace the fixed locale overlay in `session.rs` with helper-produced locale pairs**

```rust
// crates/remote-exec-daemon/src/exec/session.rs
const NORMALIZED_ENV: [(&str, &str); 7] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("CODEX_CI", "1"),
];

fn apply_env_overlay_builder(builder: &mut CommandBuilder) {
    for (key, value) in NORMALIZED_ENV {
        builder.env(key, value);
    }
    for (key, value) in super::locale::LocaleEnvPlan::resolved().as_pairs() {
        builder.env(&key, &value);
    }
}

fn apply_env_overlay_command(command: &mut Command) {
    for (key, value) in NORMALIZED_ENV {
        command.env(key, value);
    }
    for (key, value) in super::locale::LocaleEnvPlan::resolved().as_pairs() {
        command.env(&key, &value);
    }
}
```

- [ ] **Step 4: Re-run the focused env-overlay tests**

```bash
cargo test -p remote-exec-daemon --test exec_rpc env_overlay_is_applied_in_pipe_mode -- --exact --nocapture
cargo test -p remote-exec-daemon --test exec_rpc env_overlay_is_applied_in_pty_mode -- --exact --nocapture
```

Expected: PASS, showing both spawn backends now use the shared locale plan.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/exec/session.rs crates/remote-exec-daemon/tests/exec_rpc.rs
git commit -m "feat: apply exec locale fallback overlay"
```

### Task 3: Add Deterministic Locale Fallback Coverage

**Files:**
- Modify: `crates/remote-exec-daemon/src/exec/locale.rs`
- Modify: `crates/remote-exec-daemon/tests/exec_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --tests`

**Testing approach:** `existing tests + targeted verification`
Reason: the locale helper already has pure selection tests. This task adds the narrow test seam and daemon-level coverage needed to prove fallback behavior without depending on the real machine's installed locales.

- [ ] **Step 1: Add a narrow env-var override seam for discovered locale output**

```rust
// crates/remote-exec-daemon/src/exec/locale.rs
const TEST_LOCALE_OUTPUT_ENV: &str = "REMOTE_EXEC_TEST_LOCALE_OUTPUT";

pub fn resolved() -> Self {
    if let Some(plan) = resolved_from_override_env() {
        return plan;
    }

    static CACHE: OnceLock<LocaleEnvPlan> = OnceLock::new();
    CACHE.get_or_init(resolve_locale_env_plan).clone()
}

fn resolved_from_override_env() -> Option<LocaleEnvPlan> {
    let output = std::env::var(TEST_LOCALE_OUTPUT_ENV).ok()?;
    Some(LocaleEnvPlan::from_strategy(choose_strategy(
        output.lines().map(str::trim).filter(|line| !line.is_empty()),
    )))
}
```

- [ ] **Step 2: Add daemon exec RPC tests for the hybrid fallback and the final `LANG=C` fallback**

```rust
#[tokio::test]
async fn env_overlay_prefers_lang_c_plus_lc_ctype_when_c_utf8_is_unavailable() {
    let _env = EnvOverrideGuard::set(&[(
        "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
        "fr_FR.UTF-8\nen_US.UTF-8\n",
    )])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s' \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "C|en_US.UTF-8|");
}

#[tokio::test]
async fn env_overlay_falls_back_to_lang_c_only_when_no_utf8_locale_is_available() {
    let _env = EnvOverrideGuard::set(&[(
        "REMOTE_EXEC_TEST_LOCALE_OUTPUT",
        "C\nPOSIX\nen_US.ISO8859-1\n",
    )])
    .await;
    let fixture = support::spawn_daemon("builder-a").await;

    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf '%s|%s|%s' \"$LANG\" \"$LC_CTYPE\" \"$LC_ALL\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "C||");
}
```

- [ ] **Step 3: Run the full daemon test surface**

```bash
cargo test -p remote-exec-daemon --tests
```

Expected: PASS across daemon unit tests and integration tests, including the new fallback selection coverage.

- [ ] **Step 4: Commit**

```bash
git add crates/remote-exec-daemon/src/exec/locale.rs crates/remote-exec-daemon/tests/exec_rpc.rs
git commit -m "test: cover exec locale fallback behavior"
```

## Spec Coverage Check

- Prefer `C.UTF-8` and `C.utf8` when available:
  - Covered by Task 1 pure strategy tests.
- Prefer `LANG=C` plus `LC_CTYPE=<utf8 locale>` before `LC_ALL`:
  - Covered by Task 1 strategy tests and Task 3 hybrid fallback exec test.
- Prefer English UTF-8 locales:
  - Covered by Task 1 ranking tests.
- Fall back to `LANG=C` only when no UTF-8 locale exists:
  - Covered by Task 1 ranking tests and Task 3 daemon exec test.
- Keep PTY and pipe-backed child env behavior aligned:
  - Covered by Task 2 focused pipe and PTY env-overlay tests.
- Keep tests independent from the host locale inventory:
  - Covered by Task 3 env-var-based locale discovery override seam.

## Self-Review Notes

- No placeholders remain.
- The plan keeps the locale override seam local to tests instead of turning it into a public runtime configuration feature.
- The helper must bypass the normal cached path when the override env var is present, otherwise the first resolved locale plan would leak across fallback tests.
- The plan intentionally does not expose `LastResortLcAll` through daemon exec tests yet; that behavior is still pinned by the helper-level strategy tests and can be expanded later if a concrete platform requires it.
