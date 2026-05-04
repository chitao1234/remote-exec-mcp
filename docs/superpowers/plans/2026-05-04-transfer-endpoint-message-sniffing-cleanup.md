# Transfer Endpoint Message-Sniffing Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Remove the remaining broker-side production message-sniffing fallback in transfer endpoint probing and trim the unused `http-body-util` dependency from `remote-exec-host`.

**Architecture:** Keep broker transfer endpoint probing classification based on stable RPC status/code fields instead of free-form daemon message text. Use the existing broker integration stub seam to prove that a message-only `"unknown endpoint"` response is no longer silently treated as a missing destination probe, then make the smallest production change to remove that fallback and drop the dead host dependency.

**Tech Stack:** Rust 2024, Tokio, axum test stub, rmcp broker integration tests, cargo test, cargo fmt, cargo clippy

---

### Task 1: Lock the broker classification change with a red test

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer transfer_files_auto_mode_does_not_treat_message_only_unknown_endpoint_as_missing`

**Testing approach:** `TDD`
Reason: this slice changes observable `transfer_files` behavior for a specific daemon error shape, and the broker test stub already gives a direct red-green seam.

- [ ] **Step 1: Add a failing broker integration test for the message-only fallback**

```rust
#[tokio::test]
async fn transfer_files_auto_mode_does_not_treat_message_only_unknown_endpoint_as_missing() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let source = fixture._tempdir.path().join("artifact.txt");
    std::fs::write(&source, "hello exact cp mode\n").unwrap();
    fixture
        .set_transfer_path_info_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            remote_exec_proto::rpc::RpcErrorBody {
                code: String::new(),
                message: "unknown endpoint".to_string(),
            },
        )
        .await;

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "builder-xp",
                    "path": "C:/srv/inbox"
                },
                "create_parent": true
            }),
        )
        .await;

    assert!(error.contains("unknown endpoint"));
    assert!(fixture.last_transfer_import().await.is_none());
}
```

- [ ] **Step 2: Run the focused test and confirm it fails for the current message-sniffing behavior**

Run: `cargo test -p remote-exec-broker --test mcp_transfer transfer_files_auto_mode_does_not_treat_message_only_unknown_endpoint_as_missing`
Expected: FAIL because the broker still treats the free-form `"unknown endpoint"` message as a missing/unsupported path-info probe and proceeds with the import.

### Task 2: Remove the fallback and trim the unused host dependency

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Modify: `crates/remote-exec-host/Cargo.toml`
- Modify: `Cargo.lock`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer transfer_files_auto_mode_does_not_treat_message_only_unknown_endpoint_as_missing`, `cargo test -p remote-exec-broker --test mcp_transfer transfer_files_auto_mode_treats_not_found_path_info_as_missing_exact`

**Testing approach:** `TDD`
Reason: the broker classification should be changed by the smallest code edit that turns the red test green while preserving the existing code/status-based compatibility path.

- [ ] **Step 1: Remove the production message-sniffing branch and keep only status/code classification**

```rust
fn path_info_missing_or_unsupported(err: &crate::daemon_client::DaemonClientError) -> bool {
    match err {
        crate::daemon_client::DaemonClientError::Rpc { status, code, .. } => {
            *status == reqwest::StatusCode::NOT_FOUND
                || *status == reqwest::StatusCode::METHOD_NOT_ALLOWED
                || matches!(
                    code.as_deref(),
                    Some("not_found") | Some("unknown_endpoint")
                )
        }
        _ => false,
    }
}
```

- [ ] **Step 2: Remove the unused `http-body-util` dependency from the host crate manifest**

```toml
[dependencies]
anyhow = { workspace = true }
base64 = { workspace = true }
chardetng = { workspace = true }
encoding_rs = { workspace = true }
futures-util = { workspace = true }
gethostname = { workspace = true }
globset = { workspace = true }
image = { workspace = true }
```

- [ ] **Step 3: Re-run the focused broker tests**

Run: `cargo test -p remote-exec-broker --test mcp_transfer transfer_files_auto_mode_does_not_treat_message_only_unknown_endpoint_as_missing`
Expected: PASS with the broker surfacing the stub daemon error instead of proceeding with the import.

Run: `cargo test -p remote-exec-broker --test mcp_transfer transfer_files_auto_mode_treats_not_found_path_info_as_missing_exact`
Expected: PASS with the existing code/status-based compatibility path preserved.

### Task 3: Run the required verification and commit

**Files:**
- Modify: none if the gate passes
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: this changes observable broker behavior for a public tool path and also changes workspace dependencies, so the repo’s full quality gate is required before committing.

- [ ] **Step 1: Run the focused broker suite**

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: PASS.

- [ ] **Step 2: Run the workspace gate**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 3: Commit the verified slice**

```bash
git add Cargo.lock \
  docs/superpowers/plans/2026-05-04-transfer-endpoint-message-sniffing-cleanup.md \
  crates/remote-exec-broker/src/tools/transfer/endpoints.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs \
  crates/remote-exec-host/Cargo.toml
git commit -m "refactor: remove transfer endpoint message sniffing"
```
