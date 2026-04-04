# List Targets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a broker-local `list_targets` MCP tool that returns configured target names in lexicographic order without contacting daemons.

**Architecture:** Keep the feature entirely broker-local. The broker already owns the configured target map, so the implementation only needs a new public request/result type, a small broker handler that formats text plus structured JSON, and MCP router registration. Test the public MCP surface through broker integration tests, then add a focused empty-state regression test against broker state directly.

**Tech Stack:** Rust 2024, Tokio, rmcp, serde/serde_json, schemars, cargo test

---

## File Map

- `crates/remote-exec-proto/src/public.rs`
  - Public `list_targets` input and result types.
- `crates/remote-exec-broker/src/mcp_server.rs`
  - MCP tool registration and read-only annotation for `list_targets`.
- `crates/remote-exec-broker/src/tools/mod.rs`
  - Expose the new broker-local targets handler module.
- `crates/remote-exec-broker/src/tools/targets.rs`
  - Broker-local listing logic and model-facing text formatting.
- `crates/remote-exec-broker/tests/mcp_assets.rs`
  - MCP-facing behavior tests for successful listing, sort order, and read-only advertisement.
- `crates/remote-exec-broker/tests/support/mod.rs`
  - Broker fixtures for reverse-config ordering.
- `README.md`
  - Public tool list and broker-local discovery wording.

### Task 1: Add The Public `list_targets` Tool With Broker-Facing TDD

**Files:**
- Create: `crates/remote-exec-broker/src/tools/targets.rs`
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Modify: `crates/remote-exec-broker/src/tools/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`

**Testing approach:** `TDD`
Reason: the feature is an externally visible MCP behavior with a clean broker-level seam. The missing tool can be proven first with failing broker integration tests before any production code is added.

- [ ] **Step 1: Add failing MCP tests and broker fixtures for sorted listing and read-only exposure**

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs

#[tokio::test]
async fn list_targets_returns_sorted_names_and_text_output() {
    let fixture = support::spawn_broker_with_reverse_ordered_targets().await;
    let result = fixture.call_tool("list_targets", serde_json::json!({})).await;

    assert_eq!(result.text_output, "Configured targets:\n- builder-a\n- builder-b");
    assert_eq!(
        result.structured_content,
        serde_json::json!({
            "targets": ["builder-a", "builder-b"]
        })
    );
}

#[tokio::test]
async fn list_targets_is_advertised_as_read_only() {
    let fixture = support::spawn_broker_with_stub_daemon().await;

    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams {
            meta: None,
            cursor: None,
        }))
        .await
        .expect("list tools");

    let list_targets = tools
        .tools
        .into_iter()
        .find(|tool| tool.name.as_ref() == "list_targets")
        .expect("list_targets tool");

    assert_eq!(
        list_targets
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
        Some(true)
    );
}

// crates/remote-exec-broker/tests/support/mod.rs

pub async fn spawn_broker_with_reverse_ordered_targets() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, stub_state) = spawn_stub_daemon(&certs).await;
    let dead_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-b]
base_url = "https://{dead_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-b"

[targets.builder-a]
base_url = "https://{live_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
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
```

- [ ] **Step 2: Run the focused verification and confirm the tool is missing**

Run: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`
Expected: FAIL because `list_targets` is not yet registered on the broker.

- [ ] **Step 3: Implement the public types, broker handler, and MCP registration**

```rust
// crates/remote-exec-proto/src/public.rs

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ListTargetsInput {}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListTargetsResult {
    pub targets: Vec<String>,
}

// crates/remote-exec-broker/src/tools/mod.rs

pub mod exec;
pub mod exec_intercept;
pub mod image;
pub mod patch;
pub mod targets;

// crates/remote-exec-broker/src/tools/targets.rs

use remote_exec_proto::public::{ListTargetsInput, ListTargetsResult};

use crate::mcp_server::ToolCallOutput;

pub async fn list_targets(
    state: &crate::BrokerState,
    _input: ListTargetsInput,
) -> anyhow::Result<ToolCallOutput> {
    let targets = state.targets.keys().cloned().collect::<Vec<_>>();
    let text = format_targets_text(&targets);

    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(ListTargetsResult { targets })?,
    ))
}

fn format_targets_text(targets: &[String]) -> String {
    format!("Configured targets:\n- {}", targets.join("\n- "))
}

// crates/remote-exec-broker/src/mcp_server.rs

#[tool(
    name = "list_targets",
    description = "List configured target names.",
    annotations(read_only_hint = true)
)]
async fn list_targets(
    &self,
    Parameters(input): Parameters<remote_exec_proto::public::ListTargetsInput>,
) -> Result<CallToolResult, McpError> {
    Ok(
        match crate::tools::targets::list_targets(&self.state, input).await {
            Ok(output) => output.into_call_tool_result(),
            Err(err) => format_tool_error(err),
        },
    )
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`
Expected: PASS with sorted target output and a read-only `list_targets` tool in the broker tool list.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/public.rs \
  crates/remote-exec-broker/src/mcp_server.rs \
  crates/remote-exec-broker/src/tools/mod.rs \
  crates/remote-exec-broker/src/tools/targets.rs \
  crates/remote-exec-broker/tests/mcp_assets.rs \
  crates/remote-exec-broker/tests/support/mod.rs
git commit -m "feat: add list_targets tool"
```

### Task 2: Lock Empty-Config Behavior With A Focused Broker-State Test

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Test/Verify: `cargo test -p remote-exec-broker --lib list_targets_returns_empty_text_and_array_for_empty_state -- --nocapture`

**Testing approach:** `existing tests + targeted verification`
Reason: Task 1 already added the public behavior through TDD. This task adds direct regression coverage for the empty-target branch at the broker-state level required by the spec.

- [ ] **Step 1: Add a focused broker-state regression test for an empty target map**

```rust
// crates/remote-exec-broker/src/tools/targets.rs

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use remote_exec_proto::public::ListTargetsInput;

    use super::list_targets;
    use crate::{BrokerState, session_store::SessionStore};

    #[tokio::test]
    async fn list_targets_returns_empty_text_and_array_for_empty_state() {
        let state = BrokerState {
            sessions: SessionStore::default(),
            targets: BTreeMap::new(),
        };

        let result = list_targets(&state, ListTargetsInput {}).await.unwrap();
        let call_result = result.into_call_tool_result();
        let text = call_result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(text, "No configured targets.");
        assert_eq!(
            call_result.structured_content,
            Some(serde_json::json!({ "targets": [] }))
        );
    }
}
```

- [ ] **Step 2: Run the focused verification and capture the current mismatch**

Run: `cargo test -p remote-exec-broker --lib list_targets_returns_empty_text_and_array_for_empty_state -- --nocapture`
Expected: FAIL because the first-cut formatter emits `Configured targets:` for an empty list instead of the required `No configured targets.`

- [ ] **Step 3: Implement the empty-list text branch in the broker-local formatter**

```rust
// crates/remote-exec-broker/src/tools/targets.rs

fn format_targets_text(targets: &[String]) -> String {
    if targets.is_empty() {
        return "No configured targets.".to_string();
    }

    format!("Configured targets:\n- {}", targets.join("\n- "))
}
```

- [ ] **Step 4: Run the post-change verification**

Run:
```bash
cargo test -p remote-exec-broker --lib list_targets_returns_empty_text_and_array_for_empty_state -- --nocapture
cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture
```
Expected: both commands PASS, confirming the empty-state branch and the public MCP behavior still match the spec.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/targets.rs
git commit -m "test: lock list_targets empty-state behavior"
```

### Task 3: Document The Tool And Run The Full Quality Gate

**Files:**
- Modify: `README.md`
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: this task is documentation plus final release verification. The behavior is already covered by Tasks 1 and 2, so the right move is to document it and run the repo quality gate.

- [ ] **Step 1: Update the public README to advertise broker-local target discovery**

```md
## Supported tools

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

## Architecture

- Agents can call `list_targets` to discover configured logical target names.
- Machine-local tools still require an explicit `target`.

## Current status

- Core remote tools are implemented: `list_targets`, `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
- Broker target discovery is static and config-based; it does not depend on daemon reachability.
```

- [ ] **Step 2: Re-run the focused broker test after the README change**

Run: `cargo test -p remote-exec-broker --test mcp_assets list_targets -- --nocapture`
Expected: PASS.

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
git commit -m "docs: document list_targets tool"
```
