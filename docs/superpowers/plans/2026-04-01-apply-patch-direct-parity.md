# Apply Patch Direct Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring direct `apply_patch` calls in line with the updated compatibility notes by returning empty structured success content and front-loading semantic verification before any filesystem mutation starts.

**Architecture:** Keep the existing MCP function-tool entry point and daemon in-process patch engine. Make the broker return summary text plus `{}` for direct `apply_patch`, then split daemon patch handling into parse, verify, and execute phases so predictable semantic failures happen before the first write while true execution-time failures can still leave partial effects.

**Tech Stack:** Rust 2024, Tokio, Axum, rmcp, serde/schemars, cargo test

---

## File Map

- Modify: `crates/remote-exec-broker/src/tools/patch.rs`
  Responsibility: return direct-tool success as summary text plus empty structured content.
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
  Responsibility: pin the broker-visible direct `apply_patch` result shape.
- Modify: `crates/remote-exec-proto/src/public.rs`
  Responsibility: remove the no-longer-used `ApplyPatchResult` public type.
- Create: `crates/remote-exec-daemon/src/patch/verify.rs`
  Responsibility: build verified patch actions before execution begins.
- Modify: `crates/remote-exec-daemon/src/patch/mod.rs`
  Responsibility: orchestrate parse, verify, execute, and summary formatting.
- Modify: `crates/remote-exec-daemon/tests/patch_rpc.rs`
  Responsibility: prove semantic verification failures happen before mutation and preserve partial effects for true execution-time failures.

### Task 1: Align Broker `apply_patch` Success Shape

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/patch.rs:1-27`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs:3-23`
- Modify: `crates/remote-exec-proto/src/public.rs:49-62`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets apply_patch_returns_plain_text_plus_empty_structured_content -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: this is a small, externally visible broker contract change with a direct integration-test seam.

- [ ] **Step 1: Replace the broker test with the new direct-tool success expectation**

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs
#[tokio::test]
async fn apply_patch_returns_plain_text_plus_empty_structured_content() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n",
                "workdir": "."
            }),
        )
        .await;

    assert!(
        result
            .text_output
            .contains("Success. Updated the following files:")
    );
    assert_eq!(result.structured_content, serde_json::json!({}));
}
```

- [ ] **Step 2: Run the focused verification and confirm it fails first**

Run: `cargo test -p remote-exec-broker --test mcp_assets apply_patch_returns_plain_text_plus_empty_structured_content -- --exact --nocapture`
Expected: FAIL because the broker still serializes `{ "target": "...", "output": "..." }`.

- [ ] **Step 3: Return `{}` from the broker and remove the unused structured result type**

```rust
// crates/remote-exec-broker/src/tools/patch.rs
use remote_exec_proto::public::ApplyPatchInput;
use remote_exec_proto::rpc::PatchApplyRequest;

use crate::mcp_server::ToolCallOutput;

pub async fn apply_patch(
    state: &crate::BrokerState,
    input: ApplyPatchInput,
) -> anyhow::Result<ToolCallOutput> {
    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
    let response = target
        .client
        .patch_apply(&PatchApplyRequest {
            patch: input.input,
            workdir: input.workdir,
        })
        .await?;

    Ok(ToolCallOutput::text_and_structured(
        response.output,
        serde_json::json!({}),
    ))
}

// crates/remote-exec-proto/src/public.rs
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchInput {
    pub target: String,
    pub input: String,
    #[serde(default)]
    pub workdir: Option<String>,
}
```

- [ ] **Step 4: Run the focused broker verification again**

Run: `cargo test -p remote-exec-broker --test mcp_assets apply_patch_returns_plain_text_plus_empty_structured_content -- --exact --nocapture`
Expected: PASS, proving direct `apply_patch` now returns summary text plus empty structured content.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/patch.rs \
        crates/remote-exec-broker/tests/mcp_assets.rs \
        crates/remote-exec-proto/src/public.rs
git commit -m "fix: align direct apply_patch broker output"
```

### Task 2: Add Daemon Pre-Verification Before Mutation

**Files:**
- Create: `crates/remote-exec-daemon/src/patch/verify.rs`
- Modify: `crates/remote-exec-daemon/src/patch/mod.rs:1-101`
- Modify: `crates/remote-exec-daemon/tests/patch_rpc.rs:155-188`
- Test/Verify: `cargo test -p remote-exec-daemon --test patch_rpc later_verification_failures_do_not_mutate_earlier_files -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: the missing behavior is an externally observable mutation-order regression, and the daemon RPC test already exercises the public patch surface directly.

- [ ] **Step 1: Replace the current partial-mutation test with a verification-timing regression**

```rust
// crates/remote-exec-daemon/tests/patch_rpc.rs
#[tokio::test]
async fn later_verification_failures_do_not_mutate_earlier_files() {
    let fixture = support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: missing.txt\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "before\n",
    );
}
```

- [ ] **Step 2: Run the focused daemon verification and confirm it fails first**

Run: `cargo test -p remote-exec-daemon --test patch_rpc later_verification_failures_do_not_mutate_earlier_files -- --exact --nocapture`
Expected: FAIL because the current daemon updates `first.txt` before it notices that `missing.txt` does not exist.

- [ ] **Step 3: Split patch handling into verify and execute phases**

```rust
// crates/remote-exec-daemon/src/patch/verify.rs
use std::path::{Path, PathBuf};

use tokio::fs;

use super::engine;
use super::parser::PatchAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedAction {
    Add {
        path: PathBuf,
        content: String,
        summary_path: String,
    },
    Delete {
        path: PathBuf,
        summary_path: String,
    },
    Update {
        source_path: PathBuf,
        destination_path: PathBuf,
        content: String,
        summary_path: String,
        remove_source: bool,
    },
}

pub async fn verify_actions(cwd: &Path, actions: Vec<PatchAction>) -> anyhow::Result<Vec<VerifiedAction>> {
    let mut verified = Vec::with_capacity(actions.len());

    for action in actions {
        match action {
            PatchAction::Add { path, lines } => {
                let absolute_path = cwd.join(&path);
                verified.push(VerifiedAction::Add {
                    summary_path: display_relative(cwd, &absolute_path),
                    path: absolute_path,
                    content: ensure_trailing_newline(lines.join("\n")),
                });
            }
            PatchAction::Delete { path } => {
                let absolute_path = cwd.join(&path);
                fs::metadata(&absolute_path).await?;
                verified.push(VerifiedAction::Delete {
                    summary_path: display_relative(cwd, &absolute_path),
                    path: absolute_path,
                });
            }
            PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let source_path = cwd.join(&path);
                let current = fs::read_to_string(&source_path).await?;
                let destination_path = move_to
                    .as_ref()
                    .map(|destination| cwd.join(destination))
                    .unwrap_or_else(|| source_path.clone());
                let remove_source = move_to.is_some() && destination_path != source_path;
                let content = ensure_trailing_newline(engine::apply_hunks(&current, &hunks)?);

                verified.push(VerifiedAction::Update {
                    source_path,
                    destination_path: destination_path.clone(),
                    content,
                    summary_path: display_relative(cwd, &destination_path),
                    remove_source,
                });
            }
        }
    }

    Ok(verified)
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn display_relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

// crates/remote-exec-daemon/src/patch/mod.rs
mod engine;
pub mod parser;
mod verify;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse, RpcErrorBody};

use crate::AppState;

pub async fn apply_patch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref())
        .map_err(crate::exec::internal_error)?;
    let actions = parser::parse_patch(&req.patch)
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
    let verified = verify::verify_actions(&cwd, actions)
        .await
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;
    let summary = execute_verified_actions(verified)
        .await
        .map_err(|err| crate::exec::rpc_error("patch_failed", err.to_string()))?;

    Ok(Json(PatchApplyResponse {
        output: format!(
            "Success. Updated the following files:\n{}\n",
            summary.join("\n")
        ),
    }))
}

async fn execute_verified_actions(
    actions: Vec<verify::VerifiedAction>,
) -> anyhow::Result<Vec<String>> {
    let mut summary = Vec::with_capacity(actions.len());

    for action in actions {
        match action {
            verify::VerifiedAction::Add {
                path,
                content,
                summary_path,
            } => {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, content).await?;
                summary.push(format!("A {summary_path}"));
            }
            verify::VerifiedAction::Delete { path, summary_path } => {
                tokio::fs::remove_file(&path).await?;
                summary.push(format!("D {summary_path}"));
            }
            verify::VerifiedAction::Update {
                source_path,
                destination_path,
                content,
                summary_path,
                remove_source,
            } => {
                if let Some(parent) = destination_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&destination_path, content).await?;
                if remove_source {
                    tokio::fs::remove_file(&source_path).await?;
                }
                summary.push(format!("M {summary_path}"));
            }
        }
    }

    Ok(summary)
}
```

- [ ] **Step 4: Run focused and broad daemon verification**

Run: `cargo test -p remote-exec-daemon --test patch_rpc later_verification_failures_do_not_mutate_earlier_files -- --exact --nocapture`
Expected: PASS, proving semantic verification failures now happen before mutation begins.

Run: `cargo test -p remote-exec-daemon --test patch_rpc -- --nocapture`
Expected: PASS for the existing EOF-marker, repeated-context, and overwrite tests after the refactor.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/patch/mod.rs \
        crates/remote-exec-daemon/src/patch/verify.rs \
        crates/remote-exec-daemon/tests/patch_rpc.rs
git commit -m "refactor: verify apply_patch actions before execution"
```

### Task 3: Tighten Delete Verification And Lock In Regression Coverage

**Files:**
- Modify: `crates/remote-exec-daemon/src/patch/verify.rs`
- Modify: `crates/remote-exec-daemon/tests/patch_rpc.rs:1-222`
- Test/Verify: `cargo test -p remote-exec-daemon --test patch_rpc -- --nocapture`

**Testing approach:** `TDD`
Reason: the remaining gaps are externally visible ordering bugs around delete validation and need integration tests to prove they happen before mutation, while still preserving partial effects for true execution-time failures.

- [ ] **Step 1: Add the remaining daemon regression tests**

```rust
// crates/remote-exec-daemon/tests/patch_rpc.rs
#[tokio::test]
async fn delete_directory_is_rejected_before_earlier_mutation() {
    let fixture = support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::create_dir(fixture.workdir.join("nested")).await.unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: nested\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "before\n",
    );
}

#[tokio::test]
async fn non_utf8_update_source_is_rejected_before_earlier_mutation() {
    let fixture = support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("binary.txt"), vec![0xff, 0xfe, 0xfd])
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Update File: binary.txt\n",
                    "@@\n",
                    "-old\n",
                    "+new\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "before\n",
    );
}

#[tokio::test]
async fn non_utf8_delete_source_is_rejected_before_earlier_mutation() {
    let fixture = support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("binary.txt"), vec![0xff, 0xfe, 0xfd])
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: binary.txt\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "before\n",
    );
}

#[tokio::test]
async fn execution_failures_do_not_roll_back_earlier_file_changes() {
    let fixture = support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("blocked"), "not a directory\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Add File: blocked/second.txt\n",
                    "+hello\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
    assert!(std::fs::metadata(fixture.workdir.join("blocked/second.txt")).is_err());
}
```

- [ ] **Step 2: Run the daemon suite and confirm the delete-verification gaps fail first**

Run: `cargo test -p remote-exec-daemon --test patch_rpc -- --nocapture`
Expected: FAIL in the new directory-delete and non-UTF-8-delete tests because Task 2 only proved delete existence, not file-kind or UTF-8 readability.

- [ ] **Step 3: Tighten delete verification so those failures happen before execution starts**

```rust
// crates/remote-exec-daemon/src/patch/verify.rs
pub async fn verify_actions(cwd: &Path, actions: Vec<PatchAction>) -> anyhow::Result<Vec<VerifiedAction>> {
    let mut verified = Vec::with_capacity(actions.len());

    for action in actions {
        match action {
            PatchAction::Add { path, lines } => {
                let absolute_path = cwd.join(&path);
                verified.push(VerifiedAction::Add {
                    summary_path: display_relative(cwd, &absolute_path),
                    path: absolute_path,
                    content: ensure_trailing_newline(lines.join("\n")),
                });
            }
            PatchAction::Delete { path } => {
                let absolute_path = cwd.join(&path);
                let metadata = fs::metadata(&absolute_path).await?;
                anyhow::ensure!(
                    metadata.is_file(),
                    "`{}` is not a file",
                    display_relative(cwd, &absolute_path)
                );
                let _ = fs::read_to_string(&absolute_path).await?;
                verified.push(VerifiedAction::Delete {
                    summary_path: display_relative(cwd, &absolute_path),
                    path: absolute_path,
                });
            }
            PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let source_path = cwd.join(&path);
                let current = fs::read_to_string(&source_path).await?;
                let destination_path = move_to
                    .as_ref()
                    .map(|destination| cwd.join(destination))
                    .unwrap_or_else(|| source_path.clone());
                let remove_source = move_to.is_some() && destination_path != source_path;
                let content = ensure_trailing_newline(engine::apply_hunks(&current, &hunks)?);

                verified.push(VerifiedAction::Update {
                    source_path,
                    destination_path: destination_path.clone(),
                    content,
                    summary_path: display_relative(cwd, &destination_path),
                    remove_source,
                });
            }
        }
    }

    Ok(verified)
}
```

- [ ] **Step 4: Run the full daemon patch suite again**

Run: `cargo test -p remote-exec-daemon --test patch_rpc -- --nocapture`
Expected: PASS, including the new directory-delete, non-UTF-8 update/delete, and execution-time partial-failure coverage.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/patch/verify.rs \
        crates/remote-exec-daemon/tests/patch_rpc.rs
git commit -m "fix: front-load apply_patch delete validation"
```
