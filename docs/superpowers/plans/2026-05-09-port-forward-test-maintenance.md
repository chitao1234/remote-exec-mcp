# Port Forward Test Maintenance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Clean and tighten port-forward and cross-target e2e tests without changing production behavior.

**Architecture:** Keep coverage in the crate that runs it, reduce duplicated test harness code, and add only high-signal public-path assertions for v4-only tunnel behavior. Port-forward tests remain split by layer: daemon RPC tests for raw tunnel semantics, broker integration tests for MCP/public tool behavior, C++ broker tests for real C++ daemon parity, and cross-target e2e tests for multi-process target isolation.

**Tech Stack:** Rust 2024 workspace, Tokio integration tests, rmcp broker client harness, C++ daemon make-based tests, existing `serde_json` public tool payloads.

---

### Task 1: Move Cross-Target E2E Tests Under Broker Tests

**Files:**
- Modify: `crates/remote-exec-broker/tests/multi_target.rs`
- Create: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Delete: `tests/e2e/multi_target.rs`
- Delete: `tests/e2e/support/mod.rs`
- Track: `docs/superpowers/specs/2026-05-09-port-forward-test-maintenance-design.md`
- Track: `docs/superpowers/plans/2026-05-09-port-forward-test-maintenance.md`

**Testing approach:** existing tests + targeted verification.
Reason: This is a file layout cleanup. Behavior should be identical after moving the same test bodies and support code.

- [ ] **Step 1: Replace the wrapper file with the moved test body**

Run:

```bash
cp tests/e2e/multi_target.rs /tmp/multi_target.rs
python - <<'PY'
from pathlib import Path
p = Path('/tmp/multi_target.rs')
s = p.read_text()
s = s.replace('mod support;\n', '#[path = "multi_target/support.rs"]\nmod support;\n', 1)
p.write_text(s)
PY
```

Then use `apply_patch` to replace `crates/remote-exec-broker/tests/multi_target.rs` with `/tmp/multi_target.rs` contents. Do not use shell redirection to write the repo file.

- [ ] **Step 2: Move support code into the broker test subdirectory**

Run:

```bash
mkdir -p crates/remote-exec-broker/tests/multi_target
cp tests/e2e/support/mod.rs /tmp/multi_target_support.rs
```

Then use `apply_patch` to create `crates/remote-exec-broker/tests/multi_target/support.rs` from `/tmp/multi_target_support.rs` contents.

- [ ] **Step 3: Delete the old root e2e files**

Use `apply_patch` to delete:

```text
tests/e2e/multi_target.rs
tests/e2e/support/mod.rs
```

- [ ] **Step 4: Verify the moved test target**

Run:

```bash
cargo test -p remote-exec-broker --test multi_target -- --nocapture
```

Expected: all `multi_target` tests pass. This proves the moved module and support path compile and run.

- [ ] **Step 5: Commit**

```bash
git add \
  crates/remote-exec-broker/tests/multi_target.rs \
  crates/remote-exec-broker/tests/multi_target/support.rs \
  tests/e2e/multi_target.rs \
  tests/e2e/support/mod.rs \
  docs/superpowers/specs/2026-05-09-port-forward-test-maintenance-design.md \
  docs/superpowers/plans/2026-05-09-port-forward-test-maintenance.md
git commit -m "test: move cross-target e2e under broker tests"
```

### Task 2: Consolidate Broker Port-Forward Test Helpers

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Testing approach:** existing tests + targeted verification.
Reason: This is harness cleanup in an existing integration file. It should not add behavior by itself.

- [ ] **Step 1: Add small helper functions for common public tool calls**

At the bottom of `crates/remote-exec-broker/tests/mcp_forward_ports.rs`, near the existing helper functions, add these helpers:

```rust
async fn open_forward(
    fixture: &support::fixture::BrokerFixture,
    listen_side: &str,
    connect_side: &str,
    listen_endpoint: &str,
    connect_endpoint: impl ToString,
    protocol: &str,
) -> support::fixture::ToolResult {
    fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": listen_side,
                "connect_side": connect_side,
                "forwards": [{
                    "listen_endpoint": listen_endpoint,
                    "connect_endpoint": connect_endpoint.to_string(),
                    "protocol": protocol
                }]
            }),
        )
        .await
}

async fn open_tcp_forward(
    fixture: &support::fixture::BrokerFixture,
    listen_side: &str,
    connect_side: &str,
    connect_endpoint: impl ToString,
) -> support::fixture::ToolResult {
    open_forward(
        fixture,
        listen_side,
        connect_side,
        "127.0.0.1:0",
        connect_endpoint,
        "tcp",
    )
    .await
}

async fn open_udp_forward(
    fixture: &support::fixture::BrokerFixture,
    listen_side: &str,
    connect_side: &str,
    connect_endpoint: impl ToString,
) -> support::fixture::ToolResult {
    open_forward(
        fixture,
        listen_side,
        connect_side,
        "127.0.0.1:0",
        connect_endpoint,
        "udp",
    )
    .await
}

async fn close_forward(
    fixture: &support::fixture::BrokerFixture,
    forward_id: impl ToString,
) -> support::fixture::ToolResult {
    fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id.to_string()]
            }),
        )
        .await
}

async fn list_forward(
    fixture: &support::fixture::BrokerFixture,
    forward_id: &str,
) -> serde_json::Value {
    let list = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "list",
                "forward_ids": [forward_id]
            }),
        )
        .await;
    list.structured_content["forwards"][0].clone()
}
```

- [ ] **Step 2: Convert repeated simple open/list/close calls to helpers**

In `mcp_forward_ports.rs`, replace repeated simple JSON calls that exactly open one TCP/UDP forward on `127.0.0.1:0` with `open_tcp_forward` or `open_udp_forward`. Replace repeated single-forward close calls with `close_forward`. Replace repeated single-forward list calls in helper functions with `list_forward`.

Do not convert these cases:

- multi-forward open payloads,
- negative tests with intentionally malformed or unusual payloads,
- opens that use an occupied explicit listen endpoint,
- tests where inline JSON is clearer because the exact request shape is the subject.

- [ ] **Step 3: Refactor polling helpers to call `list_forward`**

Update these helpers to remove duplicated `fixture.call_tool("forward_ports", { action: "list" })` bodies:

```text
wait_for_forward_status
wait_for_forward_phase
wait_for_forward_side_health
wait_for_udp_drop_count
wait_for_tcp_drop_count
wait_for_active_tcp_streams
wait_for_forward_ready_after_reconnect
```

Each loop should call `let entry = list_forward(fixture, forward_id).await;` and keep its existing condition and failure message.

- [ ] **Step 4: Verify broker port-forward tests**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports -- --nocapture
```

Expected: all `mcp_forward_ports` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "test: consolidate broker port forward helpers"
```

### Task 3: Merge Duplicate Close-Cleanup Failure Tests

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Testing approach:** existing tests + targeted verification.
Reason: This trims redundant broker integration tests while keeping both externally visible assertions.

- [ ] **Step 1: Merge the two close-cleanup failure tests**

In `mcp_forward_ports.rs`, merge:

```text
forward_ports_close_reports_listen_cleanup_failures
forward_ports_marks_forward_failed_when_close_cleanup_fails
```

into one test named:

```rust
#[tokio::test]
async fn forward_ports_close_cleanup_failure_returns_error_and_marks_forward_failed() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(4).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    support::stub_daemon::block_session_resume(&fixture.stub_state).await;
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blackhole_addr = blackhole.local_addr().unwrap();
    drop(blackhole);

    let open = fixture
        .open_remote_tcp_forward(&blackhole_addr.to_string())
        .await;
    let forward_id = forward_id_from(&open);

    support::stub_daemon::force_close_listen_port_tunnel_transport(&fixture.stub_state).await;

    let close_error = fixture
        .call_tool_error(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id.clone()]
            }),
        )
        .await;
    assert!(
        close_error.contains("closing port forward")
            && (close_error.contains("port tunnel closed")
                || close_error.contains("resuming port tunnel session")
                || close_error.contains("waiting to resume port tunnel session")),
        "unexpected close error: {close_error}"
    );

    let forward = list_forward(&fixture, &forward_id).await;
    assert_eq!(forward["status"], "failed");
    let last_error = forward["last_error"].as_str().unwrap_or_default();
    assert!(
        last_error.contains("closing port forward")
            && (last_error.contains("port tunnel closed")
                || last_error.contains("resuming port tunnel session")
                || last_error.contains("waiting to resume port tunnel session")),
        "unexpected last_error: {last_error}"
    );
}
```

Use `force_close_listen_port_tunnel_transport` rather than the generic connect-side close helper because the behavior under test is listen cleanup on close.

- [ ] **Step 2: Verify the merged test**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_close_cleanup_failure_returns_error_and_marks_forward_failed -- --nocapture
```

Expected: the merged test passes.

- [ ] **Step 3: Verify full broker port-forward integration file**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports -- --nocapture
```

Expected: all `mcp_forward_ports` tests pass, with one fewer test than before this merge.

- [ ] **Step 4: Commit**

```bash
git add crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "test: merge port forward close cleanup coverage"
```

### Task 4: Add Public Daemon Coverage for Reserved Legacy Frames

**Files:**
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`

**Testing approach:** TDD.
Reason: This adds a missing public-path behavior assertion for the v4 cleanup: reserved legacy frame ids are decodable but unsupported through the HTTP upgrade tunnel.

- [ ] **Step 1: Add the failing test**

In `crates/remote-exec-daemon/tests/port_forward_rpc.rs`, add this test after `port_tunnel_requires_v4_header`:

```rust
#[tokio::test]
async fn port_tunnel_rejects_reserved_legacy_session_frames() {
    for frame_type in [FrameType::SessionOpen, FrameType::SessionResume] {
        let fixture = support::spawn::spawn_daemon("builder-a").await;
        let mut stream = open_tunnel(fixture.addr).await;

        write_preface(&mut stream).await.unwrap();
        write_frame(
            &mut stream,
            &json_frame(frame_type, 0, serde_json::json!({ "session_id": "legacy" })),
        )
        .await
        .unwrap();

        let error = read_frame(&mut stream).await.unwrap();
        assert_eq!(error.frame_type, FrameType::Error);
        assert_eq!(error.stream_id, 0);
        let error_meta: TunnelErrorMeta = serde_json::from_slice(&error.meta).unwrap();
        assert_eq!(error_meta.code, "invalid_port_tunnel");
        assert!(
            error_meta.message.contains("unexpected frame type"),
            "unexpected legacy-frame error message: {}",
            error_meta.message
        );
    }
}
```

- [ ] **Step 2: Run the focused test**

Run:

```bash
cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_rejects_reserved_legacy_session_frames -- --nocapture
```

Expected: pass if the current implementation already rejects these frames publicly. If it fails, inspect the error frame and adjust production code only if the implementation truly accepts a legacy frame; otherwise adjust the assertion to the truthful current public error while preserving `invalid_port_tunnel`.

- [ ] **Step 3: Run the full daemon port-forward RPC test**

Run:

```bash
cargo test -p remote-exec-daemon --test port_forward_rpc -- --nocapture
```

Expected: all port-forward RPC tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/remote-exec-daemon/tests/port_forward_rpc.rs
git commit -m "test: cover reserved legacy tunnel frames over rpc"
```

### Task 5: Clean C++ Broker Integration Port-Forward Helpers

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Testing approach:** existing tests + targeted verification.
Reason: This keeps real C++ daemon coverage but reduces repeated public-tool request boilerplate.

- [ ] **Step 1: Add helper methods on `CppDaemonBrokerFixture`**

In the `impl CppDaemonBrokerFixture` block, add these helpers near the existing `open_tcp_forward` methods:

```rust
    async fn open_forward(
        &self,
        listen_side: &str,
        connect_side: &str,
        listen_endpoint: String,
        connect_endpoint: String,
        protocol: ForwardPortProtocol,
    ) -> remote_exec_broker::client::ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Open {
                    listen_side: listen_side.to_string(),
                    connect_side: connect_side.to_string(),
                    forwards: vec![remote_exec_proto::public::ForwardPortSpec {
                        listen_endpoint,
                        connect_endpoint,
                        protocol,
                    }],
                },
            )
            .await
            .unwrap()
    }

    async fn close_forward(
        &self,
        forward_id: String,
    ) -> remote_exec_broker::client::ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::Close {
                    forward_ids: vec![forward_id],
                },
            )
            .await
            .unwrap()
    }

    async fn list_forward(&self, forward_id: String) -> remote_exec_broker::client::ToolResponse {
        self.client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::List {
                    forward_ids: vec![forward_id],
                    listen_side: None,
                    connect_side: None,
                },
            )
            .await
            .unwrap()
    }
```

Then rewrite existing `open_tcp_forward` and `open_tcp_forward_local_to_cpp` to call `open_forward`.

- [ ] **Step 2: Convert repeated close/list calls**

Replace direct `fixture.client.call_tool("forward_ports", &ForwardPortsInput::Close { ... })` calls in C++ port-forward tests with `fixture.close_forward(forward_id).await` where there is exactly one forward id. Replace the single list call in `cpp_forward_ports_reconnect_after_connect_tunnel_drop` with `fixture.list_forward(forward_id.clone()).await`.

Do not change the crashable broker fixture in this task; it owns a different client lifecycle and its explicit reopen loop is clearer as-is.

- [ ] **Step 3: Verify real C++ broker integration tests**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp -- --nocapture
```

Expected: all C++ broker port-forward tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "test: clean cpp port forward broker helpers"
```

### Task 6: Final Verification Gate

**Files:**
- Verify only unless a gate exposes a maintenance regression.

**Testing approach:** full regression verification.
Reason: The maintenance work touches test layout and integration harness code across Rust and C++ paths.

- [ ] **Step 1: Run focused port-forward and cross-target gates**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports -- --nocapture
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp -- --nocapture
cargo test -p remote-exec-broker --test multi_target -- --nocapture
cargo test -p remote-exec-daemon --test port_forward_rpc -- --nocapture
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: all focused gates pass.

- [ ] **Step 2: Run workspace quality gates**

Run:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

Expected: all commands pass with exit code 0.

- [ ] **Step 3: Inspect final status**

Run:

```bash
git status --short
git log --oneline -6
```

Expected: no uncommitted source/test/doc changes except generated ignored build artifacts. The recent commits should correspond to the tasks above.

- [ ] **Step 4: No-op commit checkpoint**

If no code changes are needed during final verification, do not create an empty commit. If a verification fix is required, commit it with:

```bash
git add <changed-files>
git commit -m "test: finalize port forward test maintenance"
```
