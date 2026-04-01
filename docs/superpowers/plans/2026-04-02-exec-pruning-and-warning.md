# Exec Pruning And Warning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement strict per-target exec session pruning and warning metadata for intercepted `apply_patch` and `60`-session threshold crossings.

**Architecture:** The daemon session store becomes responsible for protected-recent pruning and threshold crossing detection. The daemon attaches optional warning payloads to `ExecResponse`, and the broker maps both daemon warnings and local intercepted-`apply_patch` warnings into MCP `CallToolResult.meta` without changing normal text output.

**Tech Stack:** Rust 2024, Tokio, Axum, rmcp, serde, cargo test

---

## File Map

- Modify: `crates/remote-exec-daemon/src/exec/store.rs`
  - Add strict protected-recent pruning, threshold crossing detection, and an insert outcome type.
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
  - Consume the insert outcome and attach warning payloads to running `exec_command` responses.
- Modify: `crates/remote-exec-proto/src/rpc.rs`
  - Add shared exec warning payload types and optional warning fields on `ExecResponse`.
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
  - Allow both success and error tool results to carry optional MCP `meta`.
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
  - Attach warning metadata for intercepted `apply_patch` calls and forwarded daemon warnings.
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
  - Capture MCP `meta` in test results and let stub daemon responses carry exec warnings.
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
  - Add broker tests for intercepted and forwarded warnings.
- Modify: `README.md`
  - Document strict per-target pruning and warning metadata behavior.

### Task 1: Strict Daemon Store Pruning

**Files:**
- Modify: `crates/remote-exec-daemon/src/exec/store.rs`
- Test/Verify: `cargo test -p remote-exec-daemon exec::store::tests -- --nocapture`

**Testing approach:** `TDD`
Reason: The pruning and threshold behavior has a direct daemon-store unit-test seam with no broker or transport noise.

- [ ] **Step 1: Add failing store tests for protected-recent pruning and threshold crossing**

```rust
#[tokio::test]
async fn insert_protects_eight_most_recent_sessions() {
    let store = SessionStore::new(10);
    for index in 0..10 {
        store
            .insert(format!("session-{index}"), spawn_pipe_session("sleep 30"))
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    store.lock("session-9").await.expect("session-9");
    store.lock("session-8").await.expect("session-8");
    store.lock("session-7").await.expect("session-7");
    store.lock("session-6").await.expect("session-6");
    store.lock("session-5").await.expect("session-5");
    store.lock("session-4").await.expect("session-4");
    store.lock("session-3").await.expect("session-3");
    store.lock("session-2").await.expect("session-2");

    let outcome = store
        .insert("session-10".to_string(), spawn_pipe_session("sleep 30"))
        .await;

    assert!(!outcome.crossed_warning_threshold);
    assert!(store.lock("session-0").await.is_none());
    for protected in ["session-2", "session-3", "session-4", "session-5", "session-6", "session-7", "session-8", "session-9"] {
        assert!(store.lock(protected).await.is_some(), "{protected} should remain protected");
    }
}

#[tokio::test]
async fn insert_prunes_oldest_exited_non_protected_session_before_live_one() {
    let store = SessionStore::new(10);
    store
        .insert("session-0".to_string(), spawn_pipe_session("printf done"))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    for index in 1..10 {
        store
            .insert(format!("session-{index}"), spawn_pipe_session("sleep 30"))
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let outcome = store
        .insert("session-10".to_string(), spawn_pipe_session("sleep 30"))
        .await;

    assert!(!outcome.crossed_warning_threshold);
    assert!(store.lock("session-0").await.is_none());
    assert!(store.lock("session-1").await.is_some());
}

#[tokio::test]
async fn insert_reports_warning_only_when_crossing_threshold() {
    let store = SessionStore::new(64);

    for index in 0..59 {
        let outcome = store
            .insert(format!("session-{index}"), spawn_pipe_session("sleep 30"))
            .await;
        assert!(!outcome.crossed_warning_threshold, "unexpected warning at {index}");
    }

    let crossing = store
        .insert("session-59".to_string(), spawn_pipe_session("sleep 30"))
        .await;
    assert!(crossing.crossed_warning_threshold);

    let above_threshold = store
        .insert("session-60".to_string(), spawn_pipe_session("sleep 30"))
        .await;
    assert!(!above_threshold.crossed_warning_threshold);

    store.remove("session-0").await;
    store.remove("session-1").await;

    let recrossing = store
        .insert("session-61".to_string(), spawn_pipe_session("sleep 30"))
        .await;
    assert!(recrossing.crossed_warning_threshold);
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-daemon exec::store::tests -- --nocapture`
Expected: FAIL because `SessionStore::insert` does not yet return an outcome, the protected-recent policy is not implemented, and threshold crossing is not tracked.

- [ ] **Step 3: Implement strict pruning and threshold detection in the store**

```rust
const DEFAULT_SESSION_LIMIT: usize = 64;
const RECENT_PROTECTION_COUNT: usize = 8;
const WARNING_THRESHOLD: usize = 60;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InsertOutcome {
    pub crossed_warning_threshold: bool,
}

impl SessionStore {
    pub async fn insert(&self, session_id: String, session: LiveSession) -> InsertOutcome {
        let crossed_warning_threshold = self.crosses_warning_threshold().await;
        self.prune_for_insert().await;
        self.inner.write().await.insert(
            session_id,
            SessionEntry {
                session: Arc::new(Mutex::new(session)),
                last_touched_at: Instant::now(),
            },
        );
        InsertOutcome {
            crossed_warning_threshold,
        }
    }

    async fn crosses_warning_threshold(&self) -> bool {
        let current_len = self.inner.read().await.len();
        current_len < WARNING_THRESHOLD && current_len + 1 >= WARNING_THRESHOLD
    }

    fn protected_recent_count(&self) -> usize {
        self.limit.saturating_sub(1).min(RECENT_PROTECTION_COUNT)
    }

    async fn prune_for_insert(&self) {
        loop {
            let snapshot = {
                let sessions = self.inner.read().await;
                if sessions.len() < self.limit {
                    return;
                }
                let mut snapshot = sessions
                    .iter()
                    .map(|(session_id, entry)| Candidate {
                        session_id: session_id.clone(),
                        session: entry.session.clone(),
                        last_touched_at: entry.last_touched_at,
                    })
                    .collect::<Vec<_>>();
                snapshot.sort_by_key(|candidate| candidate.last_touched_at);
                snapshot
            };

            let protected = self.protected_recent_count();
            let prunable = &snapshot[..snapshot.len().saturating_sub(protected)];
            let victim = self.find_oldest_exited(prunable).await.or_else(|| prunable.first().cloned());

            let Some(victim) = victim else {
                return;
            };

            let removed = {
                let mut sessions = self.inner.write().await;
                let is_current = sessions
                    .get(&victim.session_id)
                    .is_some_and(|current| Arc::ptr_eq(&current.session, &victim.session));
                if is_current {
                    sessions.remove(&victim.session_id)
                } else {
                    None
                }
            };

            if let Some(removed) = removed {
                let mut guard = removed.session.lock_owned().await;
                let _ = guard.terminate().await;
                return;
            }
        }
    }
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon exec::store::tests -- --nocapture`
Expected: PASS with the new protected-recent and threshold-crossing tests green alongside the existing store tests.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/exec/store.rs
git commit -m "feat: align daemon exec session pruning"
```

### Task 2: Broker Meta Support And Intercepted `apply_patch` Warnings

**Files:**
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Test/Verify: `cargo test -p remote-exec-broker exec_command_intercepted_apply_patch_warning -- --nocapture`

**Testing approach:** `TDD`
Reason: The intercepted warning surface is pure broker behavior and can be driven directly from broker integration tests.

- [ ] **Step 1: Add failing broker tests and capture MCP meta in the fixture**

```rust
pub struct ToolResult {
    pub is_error: bool,
    pub text_output: String,
    pub structured_content: serde_json::Value,
    pub raw_content: Vec<serde_json::Value>,
    pub meta: Option<serde_json::Value>,
}

impl ToolResult {
    fn from_call_tool_result(result: CallToolResult) -> Self {
        let text_output = result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        Self {
            is_error: result.is_error.unwrap_or(false),
            text_output,
            structured_content: result.structured_content.unwrap_or(serde_json::Value::Null),
            raw_content: result.content.iter().map(normalize_content).collect(),
            meta: result.meta.map(serde_json::Value::Object),
        }
    }
}

#[tokio::test]
async fn exec_command_intercepted_apply_patch_warning_success_in_meta() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let patch = "*** Begin Patch\n*** Add File: warning.txt\n+warning\n*** End Patch\n";

    let result = fixture
        .raw_tool_result(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("apply_patch '{patch}'"),
            }),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(
        result.meta.as_ref().unwrap()["warnings"][0]["code"],
        "apply_patch_via_exec_command"
    );
}

#[tokio::test]
async fn exec_command_intercepted_apply_patch_warning_error_in_meta() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .raw_tool_result(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "apply_patch 'not a patch'",
            }),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(
        result.meta.as_ref().unwrap()["warnings"][0]["message"],
        "Use apply_patch directly rather than through exec_command."
    );
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-broker exec_command_intercepted_apply_patch_warning -- --nocapture`
Expected: FAIL because broker results do not yet preserve `meta` and intercepted `apply_patch` does not attach warning metadata on success or error.

- [ ] **Step 3: Implement meta-aware broker results and intercepted warnings**

```rust
pub struct ToolCallOutput {
    pub content: Vec<Content>,
    pub structured: serde_json::Value,
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ToolCallOutput {
    pub fn text_structured_meta(
        text: String,
        structured: serde_json::Value,
        meta: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Self {
        Self {
            content: vec![Content::text(text)],
            structured,
            meta,
        }
    }

    pub fn into_call_tool_result(self) -> CallToolResult {
        CallToolResult {
            content: self.content,
            structured_content: Some(self.structured),
            is_error: Some(false),
            meta: self.meta,
        }
    }
}

fn tool_error_result(
    text: String,
    meta: Option<serde_json::Map<String, serde_json::Value>>,
) -> CallToolResult {
    CallToolResult {
        content: vec![Content::text(text)],
        structured_content: None,
        is_error: Some(true),
        meta,
    }
}

fn warning_meta(code: &str, message: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    Some(serde_json::Map::from_iter([(
        "warnings".to_string(),
        serde_json::json!([{ "code": code, "message": message }]),
    )]))
}

const APPLY_PATCH_WARNING_CODE: &str = "apply_patch_via_exec_command";
const APPLY_PATCH_WARNING_MESSAGE: &str =
    "Use apply_patch directly rather than through exec_command.";
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker exec_command_intercepted_apply_patch_warning -- --nocapture`
Expected: PASS with both the intercepted success and intercepted error meta-warning tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/mcp_server.rs crates/remote-exec-broker/src/tools/exec.rs crates/remote-exec-broker/tests/support/mod.rs crates/remote-exec-broker/tests/mcp_exec.rs
git commit -m "feat: add exec warning metadata support"
```

### Task 3: Forward Daemon Session-Pressure Warnings

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Test/Verify: `cargo test -p remote-exec-broker exec_command_forwarded_session_warning -- --nocapture`

**Testing approach:** `TDD`
Reason: The warning transport is externally observable and can be proven from broker tests while Task 1 already covers the daemon store threshold logic directly.

- [ ] **Step 1: Add a failing broker test for forwarded daemon warnings**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWarning {
    pub code: String,
    pub message: String,
}

#[derive(Clone)]
struct StubDaemonState {
    target: String,
    daemon_instance_id: String,
    exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    exec_start_calls: Arc<Mutex<usize>>,
    last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    image_read_response: Arc<Mutex<StubImageReadResponse>>,
}

#[tokio::test]
async fn exec_command_forwarded_session_warning_in_meta() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    fixture
        .set_exec_start_warnings(vec![remote_exec_proto::rpc::ExecWarning {
            code: "exec_session_limit_approaching".to_string(),
            message: "Target `builder-a` now has 60 open exec sessions.".to_string(),
        }])
        .await;

    let result = fixture
        .raw_tool_result(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(
        result.meta.as_ref().unwrap()["warnings"][0]["code"],
        "exec_session_limit_approaching"
    );
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-broker exec_command_forwarded_session_warning -- --nocapture`
Expected: FAIL because `ExecResponse` has no warning field and broker exec results do not yet translate daemon warnings into MCP metadata.

- [ ] **Step 3: Implement warning payload transport from daemon to broker**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWarning {
    pub code: String,
    pub message: String,
}

impl ExecWarning {
    pub fn session_limit_approaching(target: &str) -> Self {
        Self {
            code: "exec_session_limit_approaching".to_string(),
            message: format!("Target `{target}` now has 60 open exec sessions."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecResponse {
    pub daemon_session_id: Option<String>,
    pub daemon_instance_id: String,
    pub running: bool,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub original_token_count: Option<u32>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ExecWarning>,
}

let insert_outcome = state.sessions.insert(daemon_session_id.clone(), session).await;
let warnings = if insert_outcome.crossed_warning_threshold {
    vec![remote_exec_proto::rpc::ExecWarning::session_limit_approaching(
        &state.config.target,
    )]
} else {
    Vec::new()
};

Ok(Json(ExecResponse {
    daemon_session_id: Some(daemon_session_id),
    daemon_instance_id: state.daemon_instance_id.clone(),
    running: true,
    chunk_id: Some(chunk_id()),
    wall_time_seconds,
    exit_code: None,
    original_token_count: Some(snapshot.original_token_count),
    output: snapshot.output,
    warnings,
}))
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker exec_command_forwarded_session_warning -- --nocapture`
Expected: PASS with the forwarded warning test green and no text-output changes in the broker result.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-daemon/src/exec/mod.rs crates/remote-exec-broker/src/tools/exec.rs crates/remote-exec-broker/tests/support/mod.rs crates/remote-exec-broker/tests/mcp_exec.rs
git commit -m "feat: forward exec session pressure warnings"
```

### Task 4: Update Docs And Run The Full Quality Gate

**Files:**
- Modify: `README.md`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: This task is documentation and final verification, and the behavior seams are already covered in Tasks 1 through 3.

- [ ] **Step 1: Update the README reliability notes for pruning and warnings**

```md
## Reliability Notes

- Each daemon keeps at most `64` open exec sessions for its target machine.
- The `8` most recently used sessions on a target are protected from pruning.
- `exec_command` emits warning metadata when a target crosses the `60`-session threshold.
- Intercepted `apply_patch` inside `exec_command` emits warning metadata telling the model to use `apply_patch` directly.
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test --workspace`
Expected: PASS with all workspace unit, integration, broker, daemon, and e2e tests green.

- [ ] **Step 3: Run the formatting check**

Run: `cargo fmt --all --check`
Expected: PASS with no formatting diffs.

- [ ] **Step 4: Run the linter gate**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: describe exec pruning and warning behavior"
```
