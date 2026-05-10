# Stringly Typed Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Replace the valid stringly typed review findings #17-#20 with typed internal classifications while preserving the existing Rust/C++ wire protocol strings.

**Architecture:** Rust keeps `RpcErrorBody.code` as a wire string, but broker decision code uses a typed `RpcErrorCode` helper on `DaemonClientError`. C++ transfer code parses `source_type` and `symlink_mode` at request/test boundaries into enums, switches on enums internally, and converts back to wire strings only when writing HTTP/JSON responses.

**Tech Stack:** Rust 2024, reqwest/serde broker client, C++11-compatible daemon transfer code, GNU/BSD make shared source inventory.

---

### Task 1: Rust RPC Error Code Classification

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Test/Verify: broker unit/integration tests for exec, transfer, and port forwarding

**Testing approach:** Existing tests + small unit tests.
Reason: The public behavior is unchanged; the risk is classification regression at existing decision points.

- [x] **Step 1: Add typed RPC code helper**

Add a `RpcErrorCode` enum in `daemon_client.rs` and classify known wire strings while retaining unknown strings for display.

- [x] **Step 2: Replace broker string comparisons**

Use `err.rpc_error_code()` / `err.is_rpc_error_code(...)` at exec, transfer endpoint, and port-forward retry sites.

- [x] **Step 3: Run Rust focused verification**

Run:
`cargo test -p remote-exec-broker --lib`
`cargo test -p remote-exec-broker --test mcp_exec`
`cargo test -p remote-exec-broker --test mcp_transfer`
`cargo test -p remote-exec-broker --test mcp_forward_ports`

Expected: all pass.

- [ ] **Step 4: Commit Rust task**

Run:
`git add docs/superpowers/plans/2026-05-10-stringly-typed-review-fixes.md crates/remote-exec-broker/src/daemon_client.rs crates/remote-exec-broker/src/tools/exec.rs crates/remote-exec-broker/src/tools/transfer/endpoints.rs crates/remote-exec-broker/src/port_forward/tunnel.rs`
`git commit -m "refactor: type broker rpc errors"`

### Task 2: C++ Transfer Source/Symlink Types

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/transfer_ops.h`
- Modify: `crates/remote-exec-daemon-cpp/include/transfer_http_codec.h`
- Modify: `crates/remote-exec-daemon-cpp/include/server_request_utils.h`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_tar.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Test/Verify: C++ POSIX and XP checks

**Testing approach:** Existing transfer/route integration tests.
Reason: Wire behavior should remain stable; existing route and transfer tests verify public strings and behavior.

- [ ] **Step 1: Add C++ transfer enums and conversion helpers**

Define `TransferSourceType` and `TransferSymlinkMode`, parse wire values at boundaries, and emit wire values for headers/JSON/logs/tests.

- [ ] **Step 2: Convert transfer internals to enums**

Change transfer structs and function signatures to carry enums internally, including explicit rejection of `TransferSourceType::Multiple` on export.

- [ ] **Step 3: Update C++ tests and route logging**

Compare enum values in direct transfer tests and convert to wire strings where route JSON/headers require strings.

- [ ] **Step 4: Run C++ focused verification**

Run:
`make -C crates/remote-exec-daemon-cpp check-posix`
`make -C crates/remote-exec-daemon-cpp check-windows-xp`

Expected: all pass.

- [ ] **Step 5: Commit C++ task**

Run:
`git add crates/remote-exec-daemon-cpp/include/transfer_ops.h crates/remote-exec-daemon-cpp/include/transfer_http_codec.h crates/remote-exec-daemon-cpp/include/server_request_utils.h crates/remote-exec-daemon-cpp/src/transfer_ops.cpp crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp crates/remote-exec-daemon-cpp/src/transfer_ops_tar.cpp crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp crates/remote-exec-daemon-cpp/src/server_request_utils.cpp crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp crates/remote-exec-daemon-cpp/src/http_connection.cpp crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
`git commit -m "refactor: type cpp transfer modes"`

### Task 3: Final Verification

**Files:**
- Verify all modified files

**Testing approach:** Formatting and diff verification.
Reason: The task crosses Rust and C++ and needs final consistency checks before commit.

- [ ] **Step 1: Run final checks**

Run:
`cargo fmt --all --check`
`git diff --check`

Expected: both pass.

- [ ] **Step 2: Commit**

Run:
`git status --short`

Expected: clean status after Task 1 and Task 2 commits.
