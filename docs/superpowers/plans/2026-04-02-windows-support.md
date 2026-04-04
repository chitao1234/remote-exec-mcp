# Windows Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Windows support for broker-local and remote-target execution and transfers without changing the public MCP schemas for `exec_command`, `write_stdin`, or `transfer_files`.

**Architecture:** Keep the current broker/daemon split and add platform-aware internals instead of new public tools. Centralize path normalization and comparison rules in a shared helper module, make daemon PTY capability truthful, add Windows shell/login behavior in the daemon exec runtime, and update broker transfer validation so each endpoint is interpreted using that endpoint's OS rules instead of the broker host rules.

**Tech Stack:** Rust 2024, Tokio, axum, reqwest, rmcp, portable-pty, tar, rustls, cargo test, cargo fmt, cargo clippy

---

## File Map

- `crates/remote-exec-proto/src/lib.rs`
  - Export the new shared path-policy helper module.
- `crates/remote-exec-proto/src/path.rs`
  - Shared platform-aware absolute-path checks, separator normalization, and same-path comparison helpers for broker and daemon use.
- `crates/remote-exec-daemon/Cargo.toml`
  - Gate Unix-only dependencies so the crate builds on Windows.
- `crates/remote-exec-daemon/src/server.rs`
  - Report truthful PTY capability in `target-info`.
- `crates/remote-exec-daemon/src/exec/mod.rs`
  - Apply platform-aware login validation and reuse platform-aware shell/session helpers.
- `crates/remote-exec-daemon/src/exec/session.rs`
  - Make PTY capability queryable and fail `tty=true` cleanly when unsupported.
- `crates/remote-exec-daemon/src/exec/shell.rs`
  - Add Windows shell resolution and shell-family argv construction while preserving Unix behavior.
- `crates/remote-exec-daemon/src/exec/locale.rs`
  - Keep Unix locale discovery, but no-op locale shaping on Windows.
- `crates/remote-exec-daemon/src/transfer/archive.rs`
  - Use shared path-policy helpers and make executable-bit restoration best effort on supported OSes only.
- `crates/remote-exec-daemon/tests/health.rs`
  - Assert truthful PTY reporting instead of a hardcoded `true`.
- `crates/remote-exec-daemon/tests/exec_rpc.rs`
  - Gate Unix-only assertions, add Windows-specific exec/login coverage, and keep PTY polling coverage on both platforms where supported.
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
  - Add Windows path normalization coverage and gate Unix-only exec-bit/symlink assertions.
- `crates/remote-exec-broker/src/tools/exec_intercept.rs`
  - Recognize the documented Windows shell wrapper forms for `apply_patch` interception.
- `crates/remote-exec-broker/src/tools/transfer.rs`
  - Resolve per-endpoint path policy from local host OS or cached daemon metadata and stop applying host `Path` rules to remote Windows paths.
- `crates/remote-exec-broker/src/local_transfer.rs`
  - Normalize local Windows path separators before filesystem access and make executable restoration best effort only on Unix.
- `crates/remote-exec-broker/tests/support/mod.rs`
  - Add stub-daemon fixtures that can advertise Windows platform metadata and `supports_pty = false`.
- `crates/remote-exec-broker/tests/mcp_assets.rs`
  - Cover `list_targets` text/structured output for Windows metadata and truthful PTY reporting.
- `crates/remote-exec-broker/tests/mcp_exec.rs`
  - Add broker-surface tests for Windows shell-wrapper interception.
- `crates/remote-exec-broker/tests/mcp_transfer.rs`
  - Add broker-surface tests for Windows remote path acceptance and case-insensitive same-path comparison.
- `tests/e2e/multi_target.rs`
  - Keep cross-platform transfer coverage portable and gate Unix-only executable-bit checks.
- `README.md`
  - Remove the Linux-only claim and document Windows broker/daemon behavior, path normalization, and login limitations.
- `docs/local-system-tools.md`
  - Document the intended Windows compatibility behavior for shell family handling, login support, and path normalization.

### Task 1: Add Shared Path-Policy Helpers

**Files:**
- Modify: `crates/remote-exec-proto/src/lib.rs`
- Create: `crates/remote-exec-proto/src/path.rs`
- Test/Verify: `cargo test -p remote-exec-proto -- --nocapture`

**Testing approach:** `TDD`
Reason: path normalization and comparison rules have a clear pure-function seam and can be proven without involving broker or daemon state.

- [ ] **Step 1: Add failing shared path-policy tests in a new helper module**

```rust
// crates/remote-exec-proto/src/lib.rs

pub mod path;
pub mod public;
pub mod rpc;

// crates/remote-exec-proto/src/path.rs

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

pub fn is_absolute_for_policy(_policy: PathPolicy, _raw: &str) -> bool {
    todo!()
}

pub fn normalize_for_system(_policy: PathPolicy, _raw: &str) -> String {
    todo!()
}

pub fn same_path_for_policy(_policy: PathPolicy, _left: &str, _right: &str) -> bool {
    todo!()
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
```

- [ ] **Step 2: Run the focused verification and confirm the helper is still unimplemented**

Run: `cargo test -p remote-exec-proto -- --nocapture`
Expected: FAIL because the new helper functions still contain `todo!()`.

- [ ] **Step 3: Implement platform-aware absolute-path checks and comparison rules**

```rust
// crates/remote-exec-proto/src/path.rs

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
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-proto -- --nocapture`
Expected: PASS, with the new path-policy tests proving Windows separator handling and case-insensitive equality.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/lib.rs \
  crates/remote-exec-proto/src/path.rs
git commit -m "feat: add shared path policy helpers"
```

### Task 2: Make Daemon Exec Platform-Aware And Truthfully Report PTY Support

**Files:**
- Modify: `crates/remote-exec-daemon/Cargo.toml`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-daemon/src/exec/session.rs`
- Modify: `crates/remote-exec-daemon/src/exec/shell.rs`
- Modify: `crates/remote-exec-daemon/src/exec/locale.rs`
- Modify: `crates/remote-exec-daemon/tests/health.rs`
- Modify: `crates/remote-exec-daemon/tests/exec_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test health -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`

**Testing approach:** `TDD`
Reason: daemon exec behavior has direct RPC seams for PTY support, login rejection, shell selection, and polling behavior.

- [ ] **Step 1: Add failing daemon tests for truthful PTY reporting and Windows login behavior**

```rust
// crates/remote-exec-daemon/tests/health.rs

#[tokio::test]
async fn target_info_reports_runtime_pty_support() {
    let fixture = support::spawn_daemon("builder-a").await;
    let info = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json::<TargetInfoResponse>()
        .await
        .unwrap();

    assert_eq!(info.supports_pty, remote_exec_daemon::exec::session::supports_pty());
}

// crates/remote-exec-daemon/tests/exec_rpc.rs

#[cfg(windows)]
#[tokio::test]
async fn exec_start_rejects_login_shell_requests_on_windows() {
    let fixture = support::spawn_daemon("builder-a").await;
    let err = fixture
        .rpc_error(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo should-not-run".to_string(),
                workdir: None,
                shell: Some("cmd.exe".to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(true),
            },
        )
        .await;

    assert_eq!(err.code, "login_shell_unsupported");
}

#[cfg(windows)]
#[tokio::test]
async fn exec_start_uses_cmd_when_shell_is_omitted() {
    let fixture = support::spawn_daemon("builder-a").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "echo windows-ready".to_string(),
                workdir: None,
                shell: None,
                tty: false,
                yield_time_ms: Some(COMPLETED_COMMAND_YIELD_MS),
                max_output_tokens: None,
                login: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert!(response.output.to_ascii_lowercase().contains("windows-ready"));
}
```

- [ ] **Step 2: Run the focused verification and confirm the current Unix-only assumptions fail the new coverage**

Run: `cargo test -p remote-exec-daemon --test health -- --nocapture`
Expected: FAIL because `target-info` still hardcodes `supports_pty: true`.

Run: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`
Expected: FAIL on Windows because the daemon still applies Unix login-shell rules and shell defaults.

- [ ] **Step 3: Implement platform-aware shell resolution, login policy, locale shaping, and PTY capability**

```rust
// crates/remote-exec-daemon/Cargo.toml

[dependencies]
portable-pty = { workspace = true }

[target.'cfg(unix)'.dependencies]
nix = { workspace = true }

// crates/remote-exec-daemon/src/exec/session.rs

pub fn supports_pty() -> bool {
    NativePtySystem::default()
        .openpty(PtySize::default())
        .is_ok()
}

pub fn spawn(cmd: &[String], cwd: &std::path::Path, tty: bool) -> anyhow::Result<LiveSession> {
    if tty {
        anyhow::ensure!(supports_pty(), "tty is not supported on this host");
        spawn_pty(cmd, cwd)
    } else {
        spawn_pipe(cmd, cwd)
    }
}

// crates/remote-exec-daemon/src/exec/shell.rs

#[cfg(unix)]
pub fn platform_supports_login_shells() -> bool {
    true
}

#[cfg(windows)]
pub fn platform_supports_login_shells() -> bool {
    false
}

#[cfg(windows)]
pub fn resolve_shell(shell_override: Option<&str>) -> anyhow::Result<String> {
    if let Some(shell) = shell_override.filter(|value| !value.is_empty()) {
        return Ok(shell.to_string());
    }
    if let Some(comspec) = std::env::var("COMSPEC").ok().filter(|value| !value.is_empty()) {
        return Ok(comspec);
    }
    Ok("cmd.exe".to_string())
}

pub fn shell_argv(shell: &str, login: bool, cmd: &str) -> Vec<String> {
    let lower = shell.rsplit(['\\', '/']).next().unwrap_or(shell).to_ascii_lowercase();
    if lower == "powershell.exe" || lower == "powershell" || lower == "pwsh.exe" || lower == "pwsh" {
        let mut argv = vec![shell.to_string()];
        if !login {
            argv.push("-NoProfile".to_string());
        }
        argv.push("-Command".to_string());
        argv.push(cmd.to_string());
        return argv;
    }
    if cfg!(windows) {
        return vec![shell.to_string(), "/C".to_string(), cmd.to_string()];
    }
    if login {
        vec![shell.to_string(), "-l".to_string(), "-c".to_string(), cmd.to_string()]
    } else {
        vec![shell.to_string(), "-c".to_string(), cmd.to_string()]
    }
}

// crates/remote-exec-daemon/src/exec/locale.rs

impl LocaleEnvPlan {
    pub(crate) fn resolved() -> Self {
        #[cfg(windows)]
        {
            return LocaleEnvPlan::from_strategy(LocaleStrategy::LangCOnly);
        }

        #[cfg(not(windows))]
        {
            if let Some(plan) = resolved_from_override_env() {
                return plan;
            }

            static CACHE: OnceLock<LocaleEnvPlan> = OnceLock::new();
            return CACHE.get_or_init(resolve_locale_env_plan).clone();
        }
    }

    pub(crate) fn as_pairs(&self) -> Vec<(String, String)> {
        #[cfg(windows)]
        {
            return Vec::new();
        }

        #[cfg(not(windows))]
        {
            return match &self.strategy {
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
            };
        }
    }
}

// crates/remote-exec-daemon/src/exec/mod.rs

let login = match req.login {
    Some(true) if !crate::exec::shell::platform_supports_login_shells() => {
        return Err(rpc_error(
            "login_shell_unsupported",
            "login shells are not supported on this platform",
        ));
    }
    Some(true) if !state.config.allow_login_shell => {
        return Err(rpc_error(
            "login_shell_disabled",
            "login shells are disabled by daemon config",
        ));
    }
    Some(login) => login,
    None if crate::exec::shell::platform_supports_login_shells() => state.config.allow_login_shell,
    None => false,
};

let shell = crate::exec::shell::resolve_shell(req.shell.as_deref()).map_err(internal_error)?;
let argv = crate::exec::shell::shell_argv(&shell, login, &req.cmd);

// crates/remote-exec-daemon/src/server.rs

supports_pty: crate::exec::session::supports_pty(),
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon --test health -- --nocapture`
Expected: PASS, with `target-info` reporting the runtime PTY capability.

Run: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`
Expected: PASS, with Unix tests still green and Windows-specific tests passing on Windows hosts.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/Cargo.toml \
  crates/remote-exec-daemon/src/server.rs \
  crates/remote-exec-daemon/src/exec/mod.rs \
  crates/remote-exec-daemon/src/exec/session.rs \
  crates/remote-exec-daemon/src/exec/shell.rs \
  crates/remote-exec-daemon/src/exec/locale.rs \
  crates/remote-exec-daemon/tests/health.rs \
  crates/remote-exec-daemon/tests/exec_rpc.rs
git commit -m "feat: add windows exec runtime support"
```

### Task 3: Add Windows Shell Wrapper Interception And Platform-Aware Broker Test Fixtures

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`

**Testing approach:** `TDD`
Reason: broker interception and target-metadata formatting both have precise public-surface tests and do not require live platform-specific processes.

- [ ] **Step 1: Add failing broker tests for Windows shell wrappers and Windows metadata display**

```rust
// crates/remote-exec-broker/tests/mcp_exec.rs

#[tokio::test]
async fn exec_command_intercepts_windows_shell_wrappers() {
    let fixture = support::spawn_broker_with_stub_daemon_platform("windows", false).await;
    let patch = "*** Begin Patch\n*** Add File: wrapped.txt\n+wrapped\n*** End Patch\n";

    let cmd_result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("cmd /c apply_patch '{patch}'"),
            }),
        )
        .await;
    assert!(cmd_result.text_output.contains("Process exited with code 0"));

    let pwsh_result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("pwsh -NoProfile -Command \"apply_patch '{patch}'\""),
            }),
        )
        .await;
    assert!(pwsh_result.text_output.contains("Process exited with code 0"));
}

// crates/remote-exec-broker/tests/mcp_assets.rs

#[tokio::test]
async fn list_targets_formats_windows_metadata_and_truthful_pty_support() {
    let fixture = support::spawn_broker_with_stub_daemon_platform("windows", false).await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.text_output,
        "Configured targets:\n- builder-a: windows/x86_64, host=builder-a-host, version=0.1.0, pty=no"
    );
}
```

- [ ] **Step 2: Run the focused verification and confirm the current matcher/fixtures are still Unix-biased**

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: FAIL because the interception matcher only understands the current direct and Unix wrapper forms.

Run: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`
Expected: FAIL because the stub fixture still advertises only Linux metadata.

- [ ] **Step 3: Implement Windows wrapper parsing and reusable platform-selectable stub daemons**

```rust
// crates/remote-exec-broker/tests/support/mod.rs

pub async fn spawn_broker_with_stub_daemon_platform(
    platform: &str,
    supports_pty: bool,
) -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();
    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) =
        spawn_daemon_with_platform(&certs, ExecWriteBehavior::Success, platform, supports_pty)
            .await;
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state,
    }
}

async fn spawn_daemon_with_platform(
    certs: &TestCerts,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> (std::net::SocketAddr, StubDaemonState) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let state = stub_daemon_state("builder-a", exec_write_behavior, platform, supports_pty);
    spawn_named_daemon_on_addr(certs, addr, state.clone()).await;
    (addr, state)
}

fn stub_daemon_state(
    target: &str,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: Arc::new(Mutex::new("daemon-instance-1".to_string())),
        target_hostname: format!("{target}-host"),
        target_platform: platform.to_string(),
        target_arch: "x86_64".to_string(),
        target_supports_pty: supports_pty,
        exec_write_behavior: Arc::new(Mutex::new(exec_write_behavior)),
        exec_start_warnings: Arc::new(Mutex::new(Vec::new())),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
    }
}

// crates/remote-exec-broker/src/tools/exec_intercept.rs

fn strip_shell_wrapper(cmd: &str) -> &str {
    let trimmed = cmd.trim();
    for prefix in ["cmd /c ", "cmd.exe /c ", "powershell -Command ", "powershell -NoProfile -Command ", "pwsh -Command ", "pwsh -NoProfile -Command "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim_matches(|ch| ch == ' ' || ch == '\t' || ch == '"' || ch == '\'');
        }
    }
    trimmed
}

pub fn maybe_intercept_apply_patch(
    cmd: &str,
    workdir: Option<&str>,
) -> Option<InterceptedApplyPatch> {
    let trimmed = strip_shell_wrapper(cmd);
    if let Some(patch) = parse_direct_invocation(trimmed) {
        return Some(InterceptedApplyPatch {
            patch,
            workdir: workdir.map(ToString::to_string),
        });
    }
    let (effective_workdir, script) = split_cd_wrapper(trimmed, workdir);
    let (command_name, body) = parse_heredoc_invocation(script)?;
    if command_name != "apply_patch" && command_name != "applypatch" {
        return None;
    }

    Some(InterceptedApplyPatch {
        patch: format!("{body}\n"),
        workdir: effective_workdir,
    })
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: PASS, with the new Windows wrapper tests and the existing Unix wrapper tests both green.

Run: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`
Expected: PASS, with `list_targets` formatting `windows/x86_64` and `pty=no` when the stub advertises those capabilities.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/exec_intercept.rs \
  crates/remote-exec-broker/tests/support/mod.rs \
  crates/remote-exec-broker/tests/mcp_exec.rs \
  crates/remote-exec-broker/tests/mcp_assets.rs
git commit -m "feat: add windows broker exec compatibility"
```

### Task 4: Make Broker Transfer Validation Endpoint-Aware

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/transfer.rs`
- Modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`

**Testing approach:** `TDD`
Reason: the broker has a clear public seam for Windows path acceptance and same-path rejection without needing a real Windows host.

- [ ] **Step 1: Add failing broker transfer tests for Windows separator normalization and case-insensitive same-path detection**

```rust
// crates/remote-exec-broker/tests/mcp_transfer.rs

#[tokio::test]
async fn transfer_files_accepts_windows_remote_paths_on_non_windows_hosts() {
    let fixture = support::spawn_broker_with_stub_daemon_platform("windows", false).await;

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": "C:/Work/Artifact.txt"
                },
                "destination": {
                    "target": "builder-a",
                    "path": r"c:\work\artifact.txt"
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("source and destination must differ"));
}

#[cfg(unix)]
#[tokio::test]
async fn transfer_files_still_rejects_windows_paths_for_unix_local_endpoints() {
    let fixture = support::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": "C:/Work/Artifact.txt"
                },
                "destination": {
                    "target": "local",
                    "path": "/tmp/out.txt"
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("is not absolute"));
}
```

- [ ] **Step 2: Run the focused verification and confirm the broker still uses host `Path` rules for every endpoint**

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: FAIL because `tools/transfer.rs` still uses host `Path::is_absolute()` and host lexical normalization for every endpoint.

- [ ] **Step 3: Resolve endpoint path policy from local host OS or cached daemon metadata**

```rust
// crates/remote-exec-broker/src/tools/transfer.rs

use remote_exec_proto::path::{
    PathPolicy, linux_path_policy, same_path_for_policy, windows_path_policy, is_absolute_for_policy,
};

fn local_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn remote_policy(platform: &str) -> PathPolicy {
    match platform {
        "windows" => windows_path_policy(),
        _ => linux_path_policy(),
    }
}

async fn endpoint_policy(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<PathPolicy> {
    if endpoint.target == "local" {
        return Ok(local_policy());
    }
    let target = state.target(&endpoint.target)?;
    target.ensure_identity_verified(&endpoint.target).await?;
    let info = target
        .cached_daemon_info()
        .await
        .expect("identity verification populates cached daemon info");
    Ok(remote_policy(&info.platform))
}

async fn ensure_absolute(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<()> {
    let policy = endpoint_policy(state, endpoint).await?;
    anyhow::ensure!(
        is_absolute_for_policy(policy, &endpoint.path),
        "transfer endpoint path `{}` is not absolute",
        endpoint.path
    );
    Ok(())
}

async fn ensure_distinct_endpoints(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if source.target != destination.target {
        return Ok(());
    }
    let policy = endpoint_policy(state, source).await?;
    anyhow::ensure!(
        !same_path_for_policy(policy, &source.path, &destination.path),
        "source and destination must differ"
    );
    Ok(())
}

// crates/remote-exec-broker/src/local_transfer.rs

use remote_exec_proto::path::{normalize_for_system, windows_path_policy, linux_path_policy};

fn local_policy() -> remote_exec_proto::path::PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

let normalized_source = normalize_for_system(local_policy(), &path.display().to_string());
let source = PathBuf::from(normalized_source);
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: PASS, with remote Windows paths accepted and same-path comparison rejecting case-only/separator-only variations for Windows targets.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/transfer.rs \
  crates/remote-exec-broker/src/local_transfer.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "feat: add endpoint-aware broker transfer paths"
```

### Task 5: Make Daemon And Broker-Local Transfers Portable Across Windows And Linux

**Files:**
- Modify: `crates/remote-exec-daemon/src/transfer/archive.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Modify: `tests/e2e/multi_target.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** `characterization/integration test`
Reason: transfer correctness is easiest to prove at the archive/import/export seam and with the broker-plus-daemon end-to-end test that already exists.

- [ ] **Step 1: Add failing transfer tests for Windows path normalization and gate Unix-only assertions**

```rust
// crates/remote-exec-daemon/tests/transfer_rpc.rs

#[cfg(windows)]
#[tokio::test]
async fn import_accepts_forward_slash_windows_destination_paths() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
    tokio::fs::write(&source, "artifact\n").await.unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();
    let destination = fixture.workdir.join("release").join("artifact.txt");
    let destination_text = destination.display().to_string().replace('\\', "/");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (TRANSFER_DESTINATION_PATH_HEADER, destination_text),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(tokio::fs::read_to_string(&destination).await.unwrap(), "artifact\n");
}

#[cfg(unix)]
#[tokio::test]
async fn export_file_preserves_executable_mode_in_archive_header() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("tool.sh");
    tokio::fs::write(&source, "#!/bin/sh\necho hi\n")
        .await
        .unwrap();
    let mut perms = std::fs::metadata(&source).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&source, perms).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = response.bytes().await.unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut entries = archive.entries().unwrap();
    let header = entries.next().unwrap().unwrap().header().clone();

    assert_eq!(header.mode().unwrap() & 0o111, 0o111);
}

// tests/e2e/multi_target.rs

#[cfg(unix)]
#[tokio::test]
async fn transfer_files_moves_remote_directory_between_targets_without_basename_inference() {
    let cluster = support::spawn_cluster().await;
    let source_root = cluster.daemon_a.workdir.join("dist");
    std::fs::create_dir_all(source_root.join("empty")).unwrap();
    std::fs::create_dir_all(source_root.join("bin")).unwrap();
    std::fs::write(source_root.join("bin/tool.sh"), "#!/bin/sh\necho hi\n").unwrap();
    let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();
    let destination = cluster.daemon_b.workdir.join("release");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source_root.display().to_string()
                },
                "destination": {
                    "target": "builder-b",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(destination.join("empty").is_dir());
    assert_eq!(
        std::fs::metadata(destination.join("bin/tool.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    assert!(!destination.join("dist").exists());
    assert_eq!(result.structured_content["source_type"], "directory");
}
```

- [ ] **Step 2: Run the focused verification and confirm transfer code still uses Unix-only path and permission assumptions**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: FAIL on Windows because `archive.rs` still uses host `Path` rules plus unconditional Unix permission restoration.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: FAIL after the test gating changes until the end-to-end suite is made portable again.

- [ ] **Step 3: Normalize Windows paths before filesystem access and make executable restoration Unix-only**

```rust
// crates/remote-exec-daemon/src/transfer/archive.rs

use remote_exec_proto::path::{
    is_absolute_for_policy, linux_path_policy, normalize_for_system, windows_path_policy,
};

fn host_policy() -> remote_exec_proto::path::PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn host_path(raw: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(normalize_for_system(host_policy(), raw))
}

pub async fn export_path_to_archive(path: &Path) -> anyhow::Result<ExportedArchive> {
    let source_text = path.display().to_string();
    anyhow::ensure!(
        is_absolute_for_policy(host_policy(), &source_text),
        "transfer source path `{}` is not absolute",
        source_text
    );
    let path = host_path(&source_text);
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    let source_type = if metadata.file_type().is_symlink() {
        anyhow::bail!("transfer source contains unsupported symlink `{}`", path.display());
    } else if metadata.file_type().is_file() {
        TransferSourceType::File
    } else if metadata.file_type().is_dir() {
        TransferSourceType::Directory
    } else {
        anyhow::bail!(
            "transfer source path `{}` is not a regular file or directory",
            path.display()
        );
    };
    let temp = tempfile::NamedTempFile::new()?;
    let temp_path = temp.into_temp_path();
    let archive_path = temp_path.to_path_buf();
    let source_path = path.clone();
    let source_type_for_task = source_type.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&archive_path)?;
        let mut builder = tar::Builder::new(file);
        match source_type_for_task {
            TransferSourceType::File => {
                builder.append_path_with_name(&source_path, SINGLE_FILE_ENTRY)?;
            }
            TransferSourceType::Directory => {
                builder.append_dir(".", &source_path)?;
                append_directory_entries(&mut builder, &source_path, &source_path)?;
            }
        }
        builder.finish()?;
        Ok(())
    })
    .await??;
    Ok(ExportedArchive { source_type, temp_path })
}

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    anyhow::ensure!(
        is_absolute_for_policy(host_policy(), &request.destination_path),
        "transfer destination path `{}` is not absolute",
        request.destination_path
    );
    let destination = host_path(&request.destination_path);
    let replaced = prepare_destination(&destination, request).await?;
    let archive_path = archive_path.to_path_buf();
    let request = request.clone();
    tokio::task::spawn_blocking(move || {
        extract_archive(&archive_path, &destination, &request, replaced)
    })
    .await?
}

#[cfg(unix)]
fn restore_executable_bits(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if mode & 0o111 != 0 {
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restore_executable_bits(_path: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: PASS, with Windows-specific path normalization tests on Windows and Unix executable-bit assertions still passing on Unix.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS, with the portable transfer tests green and Unix-only executable-bit checks gated correctly.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/transfer/archive.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs \
  tests/e2e/multi_target.rs
git commit -m "feat: add windows transfer support"
```

### Task 6: Update Docs And Run The Full Quality Gate

**Files:**
- Modify: `README.md`
- Modify: `docs/local-system-tools.md`
- Test/Verify: `cargo test -p remote-exec-proto -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test health -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: the core behavior changes are already covered in earlier tasks; this task aligns docs with the finished behavior and proves the workspace still passes the full quality gate.

- [ ] **Step 1: Update the operator and compatibility docs for Windows support**

```markdown
<!-- README.md -->

# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux and Windows machines.

## Reliability Notes

- `transfer_files` normalizes Windows path separators before filesystem access on Windows endpoints.
- `transfer_files` compares Windows paths case-insensitively when checking obvious same-path collisions.
- executable preservation is best effort and only restored on OSes that support executable mode bits.
- Windows targets do not support `login=true`; the daemon returns an explicit tool error instead.
- `list_targets` reports the daemon's actual `supports_pty` capability rather than assuming PTY support.

<!-- docs/local-system-tools.md -->

### Remote Windows compatibility

- Windows shells use `cmd /C` or `powershell` / `pwsh -Command` style invocation rather than Unix `-c` forms.
- Remote Windows daemons reject `login=true`; Unix login-shell semantics are not emulated.
- Remote transfer endpoints on Windows accept both `/` and `\` path separators and normalize them before filesystem access.
- Windows path equality is case-insensitive for broker-side same-path checks.
- Executable-bit preservation remains best effort only on platforms that expose executable mode bits.
```

- [ ] **Step 2: Run the focused regression suite**

Run: `cargo test -p remote-exec-proto -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-daemon --test health -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: PASS

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run the Windows-specific verification on a Windows host or CI runner**

Run: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`
Expected: PASS on Windows, including the `cmd.exe` fallback and `login_shell_unsupported` assertions.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: PASS on Windows, including the forward-slash destination normalization coverage.

- [ ] **Step 4: Run the full workspace quality gate**

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add README.md \
  docs/local-system-tools.md
git commit -m "docs: document windows platform support"
```
