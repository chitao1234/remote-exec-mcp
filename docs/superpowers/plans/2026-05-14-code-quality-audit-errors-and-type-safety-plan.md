# Code Quality Audit Errors And Type Safety Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live error-handling and type-safety issues from sections `3.*` and `4.*` of `docs/code-quality-audit.md` without changing the public wire format, MCP schemas, or established broker/daemon behavior.

**Requirements:**
- Verify every `3.*` and `4.*` audit claim against the current code and record whether it is in scope, narrowed, deferred, or stale.
- Preserve public error codes, warning codes, JSON field names, MCP output structure, and port-forward tunnel wire format.
- Prefer tighter internal typing and clearer normalization seams over broad new abstraction layers.
- Preserve current tested user-visible broker error text unless a deliberate contract change is approved; consistency fixes should favor shared internal helpers first.
- Keep the daemon and broker changes Rust-2024-compatible, and keep C++ changes within the current C++11 / XP-capable toolchain envelope.
- Continue the established execution style: medium-sized tasks, focused verification per task, no worktrees, and no empty commits.
- Do not edit `docs/code-quality-audit.md`; it remains historical input, not the live contract.

**Architecture:** Treat this as three implementation batches plus a final sweep. First, remove the meaningful panic-prone and raw-wire seams by tightening request-id/header conversion and warning-code construction without changing serialized strings. Second, create one coherent broker-side boundary for daemon/tool error normalization, including forward-compatible typed RPC codes and a simpler RPC-error decode flow. Third, tighten the remaining type-safety seams in broker port forwarding, daemon config validation, and the C++ connection manager with focused owner-local refactors rather than a cross-cutting redesign.

**Verification Strategy:** Run focused verification after each task, then finish with the repo-level gates required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `make -C crates/remote-exec-daemon-cpp check-posix`.

**Assumptions / Open Questions:**
- Audit item `3.5` is no longer accurate as written because host port-forward request rejections already flow through `logged_bad_request`; any remaining work there should be limited to typed classification or helper cleanup, not a logging redesign.
- Audit item `4.5` is stale for the current public contract because `ForwardPortsInput::{List, Close}` and the CLI intentionally support multiple `forward_ids`; do not collapse that field to a single optional string.
- The safest fix for `DaemonClientError::Rpc.code` is likely a forward-compatible typed wrapper that can preserve unknown future wire strings, not a plain `Option<RpcErrorCode>` that would discard them.
- `ValidatedDaemonConfig` will likely require updating `remote-exec-daemon` runtime entrypoints and test helpers together; confirm the exact minimal public surface during execution before changing signatures.

**Planning-Time Verification Summary:**
- `3.1`: valid and narrowed. `crates/remote-exec-daemon/src/http/request_log.rs` still uses a live-path `expect(...)`, and `crates/remote-exec-proto/src/wire.rs` still panics on missing wire mappings. Additional invariant-only `expect(...)` uses exist, but the meaningful seam is limited to request-id/header conversion and generic wire-code lookup.
- `3.2`: valid and in scope. The broker still mixes `anyhow::Result<_>` tool APIs with `Result<_, DaemonClientError>` client/target APIs and normalizes them ad hoc through `?`, `.into()`, `into_anyhow_rpc_message(...)`, `normalize_transfer_error(...)`, and tool-local wrappers.
- `3.3`: partially valid and narrowed. `write_stdin` still wraps errors in `WriteStdinToolError`, but that prefix is now part of broker test expectations. The useful fix is a shared exec-tool error-normalization seam with explicit prefix policy, not forced identical text.
- `3.4`: valid and in scope. `decode_rpc_error_strict(...)` still returns `Result<DaemonClientError, DaemonClientError>`, and `decode_rpc_error(...)` still relies on an internal `expect(...)` to enforce its non-strict policy.
- `3.5`: stale as written and out of scope as a logging issue. Port-forward host code already aliases `rpc_error(...)` to `logged_bad_request(...)`; the remaining useful cleanup is the typed-error classification work captured under `4.2`.
- `4.1`: partially valid and narrowed. `APPLY_PATCH_WARNING_CODE` is still a raw string even though `remote-exec-proto` already has a `WarningCode` enum, and `DaemonClientError::Rpc.code` is still stored as `Option<String>`. The `RECONNECT_LIMIT_EXCEEDED` claim is overstated: it is a message constant today, not a broader comparison-based code system.
- `4.2`: partially valid and in scope. The real weak seam is the broker port-forward backpressure sentinel string in `crates/remote-exec-broker/src/port_forward/tunnel.rs`; the rest of the transport classification is already based on `DaemonClientError`, `RpcErrorCode`, and `std::io::ErrorKind`.
- `4.3`: valid and in scope. `ConnectionManager` still uses `ConnectionWorkerMain` plus `void*` context, and both runtime and test callers still cast raw context pointers manually.
- `4.4`: valid and in scope. The daemon still returns bare `DaemonConfig` from `load(...)` and exposes runtime entrypoints that accept bare configs, unlike the broker’s validated wrapper pattern.
- `4.5`: invalid/stale. `PortForwardFilter.forward_ids` is intentionally multi-valued because the public list/close surfaces and internal store logic support multiple IDs.

---

### Task 1: Tighten Live Panic And Wire-Code Seams

**Intent:** Remove the meaningful live-path `expect(...)` and raw warning-code seams without changing serialized wire values or broadening the scope into unrelated invariant-only `expect(...)` sites.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/src/http/request_log.rs`
- Likely modify: `crates/remote-exec-proto/src/request_id.rs`
- Likely modify: `crates/remote-exec-proto/src/wire.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/warning.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/exec.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/exec.rs`

**Notes / constraints:**
- Cover the live portion of audit items `3.1` and `4.1`.
- Preserve existing serialized request-id values, RPC/warning wire strings, and MCP structured output.
- Prefer explicit helper methods or exhaustive conversions over generic panic-on-missing lookup where practical; do not introduce a dependency like `strum` unless execution proves the repo-local approach is clearly worse.
- Add a typed warning-code path for the exec intercepted-apply-patch warning rather than keeping a broker-local raw string constant.
- Do not spend this task on invariant-only fixed-slice or constant-literal `expect(...)` sites unless they naturally disappear while cleaning up the targeted seam.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Expect: wire-code, warning-code, and request-id behavior still passes.
- Run: `cargo test -p remote-exec-daemon --lib`
- Expect: daemon request-log and config-adjacent unit coverage still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: exec warning formatting and intercepted-apply-patch behavior still passes.

- [ ] Reconfirm the exact live-path `expect(...)` and raw warning-code seams at the current code locations
- [ ] Replace the request-id/header insertion path with a non-panicking helper or invariant-preserving conversion seam
- [ ] Tighten warning-code construction around the typed proto enum while preserving wire strings
- [ ] Limit any wire-helper changes to the verified seam needed for this batch
- [ ] Run the focused proto, daemon, and broker exec verification
- [ ] Commit with real changes only

### Task 2: Unify Broker Error Normalization Around A Typed Daemon Boundary

**Intent:** Establish one coherent broker-side normalization seam from daemon/local backend errors into tool-facing errors, while simplifying RPC error decoding and preserving current tested text output.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Likely modify: `crates/remote-exec-broker/src/local_backend.rs`
- Likely modify: `crates/remote-exec-broker/src/target/capabilities.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/image.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`

**Notes / constraints:**
- Cover audit items `3.2`, `3.3`, `3.4`, and the typed-code portion of `4.1`.
- Preserve current user-visible broker error strings unless a more explicit shared helper is intentionally parameterized to keep the existing output stable.
- Prefer a forward-compatible typed wrapper for daemon RPC codes so unknown future wire strings survive decoding and logging.
- Simplify the strict/lenient RPC error decode control flow so helpers no longer return `Result<Error, Error>` or rely on internal `expect(...)`.
- Keep target-identity clearing on transport failure unchanged.
- Do not change public MCP schema fields such as `ExecWarning.code`, `TransferWarning.code`, or `RpcErrorBody.code`; improve typing behind those strings instead.

**Verification:**
- Run: `cargo test -p remote-exec-broker --lib daemon_client::tests`
- Expect: daemon-client decode, request-id, and timeout tests still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: exec and write_stdin error formatting remains correct.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: image-read error normalization still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: transfer error normalization and path-info handling still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: CLI-facing broker errors remain stable after normalization changes.

- [ ] Reconfirm the broker error-normalization paths and tested output expectations for exec, image, and transfer
- [ ] Introduce the smallest typed daemon-error code seam that preserves unknown wire codes
- [ ] Simplify strict versus lenient RPC error decoding into a single readable control-flow shape
- [ ] Centralize tool-facing normalization so exec/image/transfer no longer each invent their own boundary behavior
- [ ] Preserve the existing write_stdin prefix policy explicitly rather than implicitly through a one-off wrapper type
- [ ] Run the focused broker verification for daemon-client, exec, assets, transfer, and CLI coverage
- [ ] Commit with real changes only

### Task 3: Tighten Remaining Port-Forward, Daemon-Config, And C++ Type Boundaries

**Intent:** Clean up the remaining verified type-safety seams in broker port forwarding, daemon config validation, and the C++ connection manager without changing public multi-ID forwarding behavior or the tunnel wire format.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/events.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Likely modify: `crates/remote-exec-daemon/src/lib.rs`
- Likely modify: `crates/remote-exec-daemon/src/server.rs`
- Likely modify: `crates/remote-exec-daemon/src/tls.rs`
- Likely modify: `crates/remote-exec-daemon/src/tls_enabled.rs`
- Likely modify: `crates/remote-exec-daemon/src/http/routes.rs`
- Likely modify: `crates/remote-exec-daemon/tests/support/spawn.rs`
- Likely modify: `crates/remote-exec-daemon/tests/support/spawn_tls.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/include/connection_manager.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp`

**Notes / constraints:**
- Cover audit items `4.2`, `4.3`, and `4.4`.
- Treat `3.5` as a verified non-task except where typed port-forward errors naturally improve the same area.
- Explicitly preserve multi-ID list/close behavior; do not collapse `PortForwardFilter.forward_ids`.
- Replace only the message-sentinel-based backpressure classification; keep the existing `DaemonClientError`, `RpcErrorCode`, and `std::io::ErrorKind` transport classification where it is already the right boundary.
- For C++ `ConnectionManager`, prefer a C++11-safe callable boundary such as `std::function<void(SOCKET)>` or an equivalent typed worker object rather than another raw pointer scheme.
- For daemon config validation, follow the broker wrapper pattern only as far as it materially clarifies ownership and call-site expectations; do not create a second parallel config model.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding behavior and reconnect handling still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker-to-C++ forwarding behavior still passes.
- Run: `cargo test -p remote-exec-daemon --lib`
- Expect: daemon config and runtime unit tests still pass after wrapper changes.
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: daemon startup validation and request-id behavior still pass.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon port-forward RPC behavior still passes.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-connection-manager`
- Expect: the connection-manager host test still passes after the typed callable cleanup.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: runtime integration still passes with the updated connection-manager API.

- [ ] Reconfirm the exact string-sentinel, daemon-config, and C++ context seams at the current code locations
- [ ] Replace the broker backpressure sentinel string with a typed internal classification path while preserving transport recovery behavior
- [ ] Introduce a validated daemon-config wrapper and thread it through the minimal runtime and test-helper surfaces
- [ ] Refactor C++ connection-manager worker startup around a typed callable boundary and update runtime/tests together
- [ ] Preserve public multi-ID forward filtering and note the `4.5` audit claim as intentionally not implemented
- [ ] Run the focused broker, daemon, and C++ verification for the touched seams
- [ ] Commit with real changes only

### Task 4: Final Sweep And Full Quality Gate

**Intent:** Confirm that the verified `3.*` and `4.*` issues were either fixed, narrowed intentionally, or explicitly left stale, and ensure the combined Rust and C++ tree remains clean.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: the code paths touched by Tasks 1 through 3

**Notes / constraints:**
- Keep this sweep focused on sections `3.*` and `4.*`; do not expand into the later audit sections during implementation.
- Reconfirm that any still-string fields are string-backed only because the public wire contract requires them, not because the internal construction path remains untyped.
- If any claim remains intentionally narrowed or stale, record that explicitly in the implementation notes rather than forcing a risky cleanup.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the full Rust workspace passes.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the touched C++ code and host tests still pass.

- [ ] Re-run searches for the verified `3.*` and `4.*` seams and confirm the final remaining code shape is intentional
- [ ] Run the required Rust workspace quality gate
- [ ] Run the relevant C++ POSIX quality gate
- [ ] Summarize which audit items were fixed, narrowed, or intentionally left stale
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
