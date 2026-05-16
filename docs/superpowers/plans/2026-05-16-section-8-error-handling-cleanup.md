# Section 8 Error Handling Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Clean up the verified section 8 error-handling inconsistencies without changing the public wire format or implementing stale audit claims literally.

**Requirements:**
- Preserve the current public MCP surface and broker-daemon wire format.
- Preserve existing port-forward protocol semantics, including v4 tunnel behavior and current tunnel error payload fields.
- Fix the verified cleanup items from `docs/code-quality-audit-2026-05-16.md` section 8: `8.2`, `8.3`, `8.4`, `8.6`, and `8.7`.
- Treat `8.1` as a narrow follow-up concern only if touched incidentally; do not redesign broker/daemon boundary error handling in this pass.
- Treat `8.5` as a targeted cleanup only where there are still clearly silent tunnel-side error sinks; do not do a repo-wide `catch (...)` campaign in this pass.
- Keep Rust daemon and C++ daemon behavior aligned where they share a contract, but do not invent new cross-language abstraction layers.

**Architecture:** This pass should tighten error handling around existing boundaries rather than redesign them. On the Rust side, the main shape change is to stop throwing away typed error metadata where the current code already has a concrete type or schema available: preserve the `winptyrs::Error` source chain, decode tunnel error frames through the shared proto type, and centralize tunnel wire code strings that are part of the protocol contract. On the RPC status path, use a small shared helper pattern to normalize invalid internal status codes with explicit logging instead of silent fallback. On the C++ side, improve Windows socket diagnostics and narrow the remaining silent tunnel-send fallbacks without changing framing or status-code behavior.

**Verification Strategy:** Run focused Rust and C++ tests for each slice as it lands, then finish with the targeted broker/daemon tests that exercise tunnel errors, local host error mapping, and C++ transport behavior. Use `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` after the Rust slices. For C++ changes, run the relevant `make -C crates/remote-exec-daemon-cpp ...` targets that cover touched transport/tunnel code.

**Assumptions / Open Questions:**
- `remote_exec_proto::port_tunnel::TunnelErrorMeta` currently requires `code: String`, while broker fallback handling today allows missing `code`; implementation should confirm whether the fallback should use a broker-local decoded wrapper or extend decoding in a way that preserves existing fallback semantics.
- The repo already tracks many plan artifacts under `docs/superpowers/plans/`, so this plan should be kept in git like the existing convention.
- `8.4` includes at least one test-only raw string today (`listener_open_failed` in broker test code). Implementation should centralize protocol identifiers that are truly part of the wire contract first, then decide whether nearby test literals should use the same constants for consistency.

---

### Task 1: Preserve typed errors and tighten Rust boundary helpers

**Intent:** Fix the concrete Rust-side error erasure issues without redesigning the full error model.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/exec/winpty.rs`
- Likely modify: `crates/remote-exec-daemon/src/rpc_error.rs`
- Likely modify: `crates/remote-exec-broker/src/local_backend.rs`
- Existing references: `crates/remote-exec-host/src/exec/support.rs`
- Existing references: `crates/remote-exec-host/src/error.rs`

**Notes / constraints:**
- Preserve the current external `HostRpcError` and `DaemonClientError` shapes.
- Invalid internal HTTP status codes should still degrade safely to `500`, but the path must emit an explicit log so programming errors are visible.
- If a small shared helper can be reused by daemon and broker-local status normalization, prefer that over duplicate `unwrap_or(...)` logic.
- Do not broaden this task into a full replacement of `anyhow` at crate boundaries.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Run: `cargo fmt --all --check`
- Expect: existing behavior remains intact, invalid-status paths are logged rather than silently swallowed, and no formatting regressions appear.

- [ ] Inspect the current `winpty` error mapping and the daemon/broker status normalization seams
- [ ] Add or update tests for preserved status fallback behavior and any helper introduced for invalid status normalization
- [ ] Replace `map_winpty_error` stringification with a source-preserving conversion
- [ ] Introduce explicit logging for invalid internal HTTP status code normalization and reuse the logic where both daemon and broker-local paths need it
- [ ] Run focused Rust verification
- [ ] Commit

### Task 2: Unify broker tunnel error decoding with shared proto types

**Intent:** Remove the ad hoc `serde_json::Value` tunnel-error parsing and make broker tunnel error handling reflect the shared schema more directly.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/events.rs`
- Likely modify: `crates/remote-exec-proto/src/port_tunnel/meta.rs` only if a narrowly-scoped compatibility helper is justified
- Existing references: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`

**Notes / constraints:**
- Preserve current broker behavior when the daemon sends malformed or incomplete tunnel error metadata.
- Avoid introducing a second parallel `TunnelErrorMeta` shape unless it is strictly needed as a bounded compatibility wrapper around the shared proto type.
- Remove the string-equality fallback sentinel in `format_terminal_tunnel_error` if the decoding path can represent fallback state explicitly.
- Keep retryability and backpressure classification behavior unchanged.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: tunnel error decoding remains compatible, reconnect and bridge behavior stay unchanged, and malformed/fallback cases are still handled deterministically.

**Demonstration snippet (optional, illustrative only):**
```rust
// Shape example only. The final implementation may use a local wrapper.
struct DecodedTunnelError {
    meta: Option<remote_exec_proto::port_tunnel::TunnelErrorMeta>,
    stream_id: u32,
    used_fallback: bool,
}
```

- [ ] Confirm the exact fallback cases that the current broker logic accepts for tunnel error frames
- [ ] Add or update tests covering valid, malformed, and fallback tunnel error metadata
- [ ] Refactor broker tunnel error decoding to use the shared proto type or a narrow wrapper around it instead of raw `serde_json::Value`
- [ ] Remove or replace the current message-string sentinel handling with an explicit state representation
- [ ] Run focused Rust port-forward verification
- [ ] Commit

### Task 3: Centralize tunnel wire identifiers that are part of the protocol contract

**Intent:** Replace ad hoc string literals for tunnel reason/code identifiers with authoritative shared constants where they are actually wire-facing.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/port_tunnel/` [confirm exact module for constants]
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`
- Likely modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming_*.cpp`
- Existing references: broker test code in `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`

**Notes / constraints:**
- Prefer constants in `remote-exec-proto` for true wire identifiers shared across Rust broker, Rust daemon tests, host tests, and C++ tests.
- Do not convert free-form human messages into constants.
- If some literals are only local test fixtures rather than protocol identifiers, keep the plan narrow and document why they remain local.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: all shared tunnel reason/code producers and consumers compile against the same constant set, with no protocol behavior change.

- [ ] Inventory the currently duplicated tunnel reason/code literals and separate wire identifiers from test-only message fixtures
- [ ] Add authoritative shared constants in the proto layer for the actual protocol identifiers
- [ ] Replace Rust and C++ call sites that should consume those constants or their local C++ equivalents generated from the same authority pattern
- [ ] Update focused tests to use the centralized identifiers where appropriate
- [ ] Run focused Rust and C++ verification
- [ ] Commit

### Task 4: Improve remaining C++ tunnel/transport diagnostics without protocol changes

**Intent:** Address the still-actionable C++ section 8 issues: raw Windows socket error integers and silent tunnel-send fallback sinks.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`

**Notes / constraints:**
- Keep Windows XP-compatible C++11 constraints intact.
- Use Windows-native message formatting for socket and resolver failures where available; avoid changing error-code classification logic.
- Narrow the scope of `8.5` to the tunnel error-send helpers and other still-silent sinks, not the already-logged worker catches.
- Logging added in fallback paths must not recursively depend on the same failing send path.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- Expect: touched C++ transport and tunnel paths continue to pass, and Windows-specific error formatting remains compile-safe on non-Windows builds.

- [ ] Inspect current Windows socket error formatting and identify the remaining tunnel-side silent catches that are still worth fixing
- [ ] Add or update tests where there is an existing seam for observable error text or fallback behavior
- [ ] Replace raw Windows numeric socket diagnostics with formatted messages compatible with XP-era APIs
- [ ] Add bounded logging or explicit rationale to the remaining silent tunnel error-send fallback paths
- [ ] Run focused C++ verification
- [ ] Commit

### Task 5: Final section 8 sweep and cross-check

**Intent:** Confirm that the implemented fixes match the verified section 8 findings and did not drift into stale audit work.

**Relevant files/components:**
- Review all files touched by Tasks 1-4
- Existing references: `docs/code-quality-audit-2026-05-16.md`

**Notes / constraints:**
- Do not expand this sweep into section 9 or unrelated audit items.
- Explicitly confirm that `8.1` and the broad version of `8.5` were intentionally not implemented literally if they remain non-issues or larger redesign topics.

**Verification:**
- Run: `cargo test --workspace`
- Run: `cargo fmt --all --check`
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: workspace quality gates stay green and the final diff matches the intended section 8 scope.

- [ ] Review the final diff against the verified section 8 findings and remove any accidental scope creep
- [ ] Run the full Rust workspace verification gates
- [ ] Run the relevant final C++ verification target
- [ ] Summarize which section 8 items were fixed, which were intentionally narrowed, and any residual follow-up work
- [ ] Commit
