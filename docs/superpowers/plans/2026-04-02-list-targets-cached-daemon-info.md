# List Targets Cached Daemon Info Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand `list_targets` so it returns a breaking object-list result with nullable cached daemon metadata and richer text summaries while remaining broker-local.

**Architecture:** Keep `list_targets` as a broker-only read of target state. The broker gains a cached reduced `target_info` subset on each target handle, refreshed on successful verification and cleared on transport or daemon-mismatch evidence. Public `list_targets` output is then derived entirely from that cached state without probing daemons at read time.

**Tech Stack:** Rust 2024, Tokio, rmcp, serde/serde_json, schemars, cargo test

---

## File Map

- `crates/remote-exec-proto/src/public.rs`
  - Breaking `list_targets` result types: per-target entry object plus nullable daemon-info subset.
- `crates/remote-exec-broker/src/lib.rs`
  - TargetHandle cache storage, startup population, refresh helpers, and cache clearing on verification transport failure.
- `crates/remote-exec-broker/src/tools/targets.rs`
  - Public `list_targets` structured output and compact text rendering.
- `crates/remote-exec-broker/src/tools/exec.rs`
  - Cache clearing on daemon-instance mismatch and forwarded transport-failure evidence.
- `crates/remote-exec-broker/src/tools/image.rs`
  - Cache clearing on transport failure after a daemon call attempt.
- `crates/remote-exec-broker/src/tools/patch.rs`
  - Cache clearing on transport failure after a daemon call attempt.
- `crates/remote-exec-broker/tests/mcp_assets.rs`
  - Public `list_targets` behavior assertions for the new result shape and text summary.
- `crates/remote-exec-broker/tests/mcp_exec.rs`
  - Cache lifecycle regression tests for mismatch and repopulation.
- `crates/remote-exec-broker/tests/support/mod.rs`
  - Stub-daemon fixture controls for target-info values and daemon-instance changes.
- `README.md`
  - Public docs for cached daemon metadata behavior.

### Task 1: Drive The Breaking `list_targets` Output Shape Through Public Tests

**Files:**
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`

**Testing approach:** `TDD`
Reason: this is a public MCP contract change with a clean broker integration seam. The failing broker tests should describe the new result shape and text rendering before any production changes.

- [ ] **Step 1: Replace the existing `list_targets` broker test expectations with the new object-list shape and text summary**

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs

#[tokio::test]
async fn list_targets_returns_cached_daemon_info_and_null_for_unavailable_targets() {
    let fixture = support::spawn_broker_with_reverse_ordered_targets().await;
    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.text_output,
        "Configured targets:\n- builder-a: linux/x86_64, host=builder-a-host, version=0.1.0, pty=yes\n- builder-b"
    );
    assert_eq!(
        result.structured_content,
        serde_json::json!({
            "targets": [
                {
                    "name": "builder-a",
                    "daemon_info": {
                        "daemon_version": "0.1.0",
                        "hostname": "builder-a-host",
                        "platform": "linux",
                        "arch": "x86_64",
                        "supports_pty": true
                    }
                },
                {
                    "name": "builder-b",
                    "daemon_info": null
                }
            ]
        })
    );
}
```

- [ ] **Step 2: Make the reverse-ordered fixture return a deterministic hostname for the live target**

```rust
// crates/remote-exec-broker/tests/support/mod.rs

fn stub_daemon_state(target: &str, exec_write_behavior: ExecWriteBehavior) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: "daemon-instance-1".to_string(),
        target_hostname: format!("{target}-host"),
        target_platform: "linux".to_string(),
        target_arch: "x86_64".to_string(),
        target_supports_pty: true,
        // existing fields...
    }
}

async fn target_info(State(state): State<StubDaemonState>) -> Json<TargetInfoResponse> {
    Json(TargetInfoResponse {
        target: state.target,
        daemon_version: "0.1.0".to_string(),
        daemon_instance_id: state.daemon_instance_id,
        hostname: state.target_hostname,
        platform: state.target_platform,
        arch: state.target_arch,
        supports_pty: state.target_supports_pty,
        supports_image_read: true,
    })
}
```

- [ ] **Step 3: Run the focused verification and confirm the old names-only result fails**

Run: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`
Expected: FAIL because `list_targets` still returns `Vec<String>` and names-only text.

- [ ] **Step 4: Implement the new public result types and broker text rendering**

```rust
// crates/remote-exec-proto/src/public.rs

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetEntry {
    pub name: String,
    pub daemon_info: Option<ListTargetDaemonInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetsResult {
    pub targets: Vec<ListTargetEntry>,
}

// crates/remote-exec-broker/src/tools/targets.rs

use remote_exec_proto::public::{
    ListTargetDaemonInfo, ListTargetEntry, ListTargetsInput, ListTargetsResult,
};

pub async fn list_targets(
    state: &crate::BrokerState,
    _input: ListTargetsInput,
) -> anyhow::Result<ToolCallOutput> {
    let targets = state
        .targets
        .iter()
        .map(|(name, handle)| ListTargetEntry {
            name: name.clone(),
            daemon_info: handle.cached_daemon_info().await.map(|info| ListTargetDaemonInfo {
                daemon_version: info.daemon_version,
                hostname: info.hostname,
                platform: info.platform,
                arch: info.arch,
                supports_pty: info.supports_pty,
            }),
        })
        .collect::<Vec<_>>();
    let text = format_targets_text(&targets);

    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(ListTargetsResult { targets })?,
    ))
}

fn format_targets_text(targets: &[ListTargetEntry]) -> String {
    if targets.is_empty() {
        return "No configured targets.".to_string();
    }

    let lines = targets
        .iter()
        .map(|target| match &target.daemon_info {
            Some(info) => format!(
                "- {}: {}/{}, host={}, version={}, pty={}",
                target.name,
                info.platform,
                info.arch,
                info.hostname,
                info.daemon_version,
                if info.supports_pty { "yes" } else { "no" }
            ),
            None => format!("- {}", target.name),
        })
        .collect::<Vec<_>>();

    format!("Configured targets:\n{}", lines.join("\n"))
}
```

- [ ] **Step 5: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`
Expected: PASS with one populated cached entry, one `daemon_info: null` entry, ascending order, and richer text output.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-proto/src/public.rs \
  crates/remote-exec-broker/src/tools/targets.rs \
  crates/remote-exec-broker/tests/mcp_assets.rs \
  crates/remote-exec-broker/tests/support/mod.rs
git commit -m "feat: enrich list_targets output"
```

### Task 2: Add Broker Cache Storage, Refresh, And Clearing Rules

**Files:**
- Modify: `crates/remote-exec-broker/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/src/tools/image.rs`
- Modify: `crates/remote-exec-broker/src/tools/patch.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`

**Testing approach:** `TDD`
Reason: the cache lifecycle is observable through broker behavior and has clear regression seams for startup population, repopulation, and invalidation on mismatch or transport evidence.

- [ ] **Step 1: Add failing broker tests for cache clearing on daemon mismatch and repopulation after later verification**

```rust
// crates/remote-exec-broker/tests/mcp_exec.rs

#[tokio::test]
async fn list_targets_clears_cached_daemon_info_after_daemon_instance_mismatch() {
    let fixture = support::spawn_broker_with_stub_daemon().await;

    let before = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert!(before.structured_content["targets"][0]["daemon_info"].is_object());

    fixture.set_stub_daemon_instance_id("daemon-instance-2").await;

    let _ = fixture
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": fixture.start_running_session().await,
                "yield_time_ms": 10
            }),
        )
        .await;

    let after = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert!(after.structured_content["targets"][0]["daemon_info"].is_null());
}

#[tokio::test]
async fn list_targets_repopulates_cached_daemon_info_after_later_successful_verification() {
    let fixture = support::spawn_broker_with_live_and_dead_targets().await;

    let before = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert!(before.structured_content["targets"][1]["daemon_info"].is_null());

    fixture.spawn_target("builder-b").await;
    let _ = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-b",
                "input": "*** Begin Patch\n*** Add File: ok.txt\n+ok\n*** End Patch\n"
            }),
        )
        .await;

    let after = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;
    assert_eq!(after.structured_content["targets"][1]["daemon_info"]["hostname"], "builder-b-host");
}
```

- [ ] **Step 2: Add focused fixture controls for daemon instance changes and session startup**

```rust
// crates/remote-exec-broker/tests/support/mod.rs

impl BrokerFixture {
    pub async fn set_stub_daemon_instance_id(&self, daemon_instance_id: &str) {
        *self.stub_state.daemon_instance_id.lock().await = daemon_instance_id.to_string();
    }

    pub async fn start_running_session(&self) -> String {
        let result = self
            .call_tool(
                "exec_command",
                serde_json::json!({
                    "target": "builder-a",
                    "cmd": "printf ready; sleep 2",
                    "tty": true,
                    "yield_time_ms": 10
                }),
            )
            .await;

        result.structured_content["session_id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}
```

- [ ] **Step 3: Run the focused verification and capture the missing cache-lifecycle behavior**

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: FAIL because the broker does not yet cache daemon info, clear it on mismatch, or repopulate it on later verification success.

- [ ] **Step 4: Implement cached daemon-info storage and lifecycle hooks**

```rust
// crates/remote-exec-broker/src/lib.rs

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
}

#[derive(Clone)]
pub struct TargetHandle {
    pub client: DaemonClient,
    expected_daemon_name: Option<String>,
    identity_verified: Arc<Mutex<bool>>,
    cached_daemon_info: Arc<Mutex<Option<CachedDaemonInfo>>>,
}

impl TargetHandle {
    fn cache_from_target_info(
        info: &remote_exec_proto::rpc::TargetInfoResponse,
    ) -> CachedDaemonInfo {
        CachedDaemonInfo {
            daemon_version: info.daemon_version.clone(),
            hostname: info.hostname.clone(),
            platform: info.platform.clone(),
            arch: info.arch.clone(),
            supports_pty: info.supports_pty,
        }
    }

    pub async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.cached_daemon_info.lock().await.clone()
    }

    pub async fn clear_cached_daemon_info(&self) {
        *self.cached_daemon_info.lock().await = None;
        *self.identity_verified.lock().await = false;
    }

    pub async fn ensure_identity_verified(&self, name: &str) -> anyhow::Result<()> {
        let mut identity_verified = self.identity_verified.lock().await;
        if *identity_verified {
            return Ok(());
        }

        match self.client.target_info().await {
            Ok(info) => {
                if let Some(expected_name) = &self.expected_daemon_name {
                    anyhow::ensure!(
                        &info.target == expected_name,
                        "target `{name}` resolved to daemon `{}` instead of `{expected_name}`",
                        info.target
                    );
                }

                *self.cached_daemon_info.lock().await = Some(Self::cache_from_target_info(&info));
                *identity_verified = true;
                Ok(())
            }
            Err(DaemonClientError::Transport(err)) => {
                *self.cached_daemon_info.lock().await = None;
                *identity_verified = false;
                Err(DaemonClientError::Transport(err).into())
            }
            Err(err) => Err(err.into()),
        }
    }
}
```

```rust
// crates/remote-exec-broker/src/tools/exec.rs

        Err(err) if err.rpc_code() == Some("unknown_session") => {
            state.sessions.remove(&record.session_id).await;
            return Err(anyhow::anyhow!(unknown_process_id_message(
                &record.session_id
            )));
        }
        Err(err) => {
            if let Ok(info) = target.client.target_info().await
                && info.daemon_instance_id != record.daemon_instance_id
            {
                target.clear_cached_daemon_info().await;
                state.sessions.remove(&record.session_id).await;
                return Err(anyhow::anyhow!(unknown_process_id_message(
                    &record.session_id
                )));
            }
            if matches!(&err, crate::daemon_client::DaemonClientError::Transport(_)) {
                target.clear_cached_daemon_info().await;
            }
            return Err(err.into());
        }
```

```rust
// crates/remote-exec-broker/src/tools/image.rs

    let response = target
        .client
        .image_read(&ImageReadRequest {
            path: input.path,
            workdir: input.workdir,
            detail: input.detail.clone(),
        })
        .await
        .map_err(|err| async {
            if matches!(err, DaemonClientError::Transport(_)) {
                target.clear_cached_daemon_info().await;
            }
            normalize_view_image_error(err)
        })
        .await?;
```

```rust
// crates/remote-exec-broker/src/tools/patch.rs

    match target
        .client
        .patch_apply(&PatchApplyRequest { patch, workdir })
        .await
    {
        Ok(response) => Ok(response.output),
        Err(err) => {
            if matches!(err, crate::daemon_client::DaemonClientError::Transport(_)) {
                target.clear_cached_daemon_info().await;
            }
            Err(err.into())
        }
    }
```

- [ ] **Step 5: Run the post-change verification**

Run:
```bash
cargo test -p remote-exec-broker --test mcp_exec -- --nocapture
cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture
```
Expected: both commands PASS, confirming the public object-list result and the cache lifecycle transitions.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-broker/src/lib.rs \
  crates/remote-exec-broker/src/tools/exec.rs \
  crates/remote-exec-broker/src/tools/image.rs \
  crates/remote-exec-broker/src/tools/patch.rs \
  crates/remote-exec-broker/tests/mcp_exec.rs \
  crates/remote-exec-broker/tests/support/mod.rs
git commit -m "feat: cache daemon info for list_targets"
```

### Task 3: Update Docs And Run The Full Quality Gate

**Files:**
- Modify: `README.md`
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: the behavior is already covered by Tasks 1 and 2. This final task documents the new contract and re-runs the full repository gate before closing the work.

- [ ] **Step 1: Update the README for cached daemon metadata in `list_targets`**

```md
## Supported tools

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

## Architecture

- Agents can call `list_targets` to discover configured logical target names and cached daemon metadata when available.
- `list_targets` is broker-local and does not probe daemons at read time.

## Current status

- Core remote tools are implemented: `list_targets`, `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
- Broker target discovery returns cached daemon metadata when the broker currently considers it usable, otherwise `daemon_info` is `null`.
```

- [ ] **Step 2: Re-run the focused broker tests after the README update**

Run:
```bash
cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture
cargo test -p remote-exec-broker --test mcp_exec -- --nocapture
```
Expected: both commands PASS.

- [ ] **Step 3: Run the full workspace quality gate**

Run:
```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```
Expected: all commands PASS cleanly.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: describe cached list_targets metadata"
```
