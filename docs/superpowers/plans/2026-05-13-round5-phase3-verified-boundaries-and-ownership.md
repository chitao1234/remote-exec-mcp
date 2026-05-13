# Round 5 Phase 3 Verified Boundaries And Ownership Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Round 5 Phase 3 ownership and boundary issues without reopening stale audit claims or violating the current architecture contract.

**Requirements:**
- Cover the verified current-code follow-up for audit items `#17` and `#18`.
- Exclude audit item `#15` from execution because the daemon crate still owns meaningful HTTP routing, auth, version enforcement, TLS binding/serving, and daemon-specific config validation; the earlier "thin pass-through" framing and cited no-op stubs are no longer accurate enough to drive a safe phase.
- Exclude audit item `#16` from execution because `AGENTS.md` explicitly treats `crates/remote-exec-proto/src/path.rs` as the live home for cross-platform path policy helpers, so moving those helpers out of `proto` would conflict with the current project contract.
- Exclude audit item `#19` from execution because `write_test_bound_addr_file` is gone from the broker and `StreamIdAllocator::set_next_for_test` is already gated behind `#[cfg(test)]`.
- Preserve public port-tunnel protocol behavior, retained listener and UDP-bind resume behavior, error-code mapping, and the current broker/daemon RPC contract.
- Keep this phase inside Rust host port-forward ownership cleanup. Do not widen it into daemon/host crate-merging work, public API changes, or earlier dedup items that belong to other phases.

**Architecture:** Execute this phase as two medium-sized host-port-forward batches plus a final sweep. First, add a single internal access layer around tunnel mode and session lookup so `tcp.rs` and `udp.rs` stop pattern-matching raw `TunnelMode` at every operational seam. Second, remove the duplicate listen-session ownership currently split between `TunnelMode::Listen` and `TunnelState` by making the session association a single tunnel-owned concern and routing callers through the new helper layer. This keeps the behavioral surface stable while reducing the ownership leakage that currently drives the repeated dispatch patterns.

**Verification Strategy:** Verify each batch with focused host port-forward coverage first, then run daemon and broker forwarding tests where the refactor touches shared runtime behavior. Finish with the Rust quality gate required for cross-cutting refactors: `cargo test --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

**Assumptions / Open Questions:**
- The earlier Phase 2 work already made audit item `#11` stale by routing both TCP read-loop ownership modes through `TcpReadLoopTarget`, but the higher-level mode/session dispatch duplication is still present.
- The safest execution order is to introduce helpers before changing `TunnelMode::Listen`, so the ownership change lands behind a stable internal call surface.
- The best home for the helper layer may be `tunnel.rs`, a new internal `access.rs`, or small helper methods on `TunnelState`; implementation should confirm which shape is clearest without inventing a broader abstraction layer.
- The current `attached_session` field name may no longer match its post-refactor role if it becomes the canonical listen-session handle; implementation should rename it if that materially improves semantic clarity.

**Planning-Time Verification Summary:**
- `#15`: partially stale and not selected for execution; the daemon crate still has thin wrappers in `lib.rs`, but the larger crate owns distinct HTTP/TLS/config behavior and the specific empty-stub evidence from the audit is gone.
- `#16`: intentionally not selected for execution; the live repository contract now places cross-platform path policy helpers in `remote-exec-proto::path`.
- `#17`: still live; `tcp.rs` and `udp.rs` still contain repeated `tunnel_mode(...)` matches for listen/connect protocol dispatch and session/socket/writer lookup.
- `#18`: still live; `TunnelMode::Listen` still carries `Arc<SessionState>` while `TunnelState` also tracks session association, so callers keep pulling session state out of the enum.
- `#19`: stale; the older broker test-only production seam is gone and the remaining allocator test seam is correctly test-gated.

---

### Task 1: Introduce A Host Port-Forward Access Layer

**Intent:** Collapse the repeated raw `TunnelMode` matching in `tcp.rs` and `udp.rs` behind a smaller internal boundary without changing tunnel behavior yet.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely create: `crates/remote-exec-host/src/port_forward/access.rs` if the helper count justifies a separate internal module
- Existing references: `crates/remote-exec-host/src/port_forward/session.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/types.rs`

**Notes / constraints:**
- Preserve the existing invalid-tunnel, unknown-stream, and closed-attachment error messages where tests already assert them.
- Keep the helper boundary internal to `remote-exec-host::port_forward`; this is an ownership cleanup, not a public API redesign.
- Prefer helpers that return typed session/socket/writer access over callback-heavy closures unless the closure shape proves materially clearer in the real code.

**Verification:**
- Run: `cargo test -p remote-exec-host port_forward::port_tunnel_tests`
- Expect: host port-tunnel coverage still passes after the helper extraction.

- [ ] Inspect the current `tunnel_mode(...)` call sites and group them by operation type: listen-session lookup, attachment lookup, TCP stream writer lookup, and UDP socket/bind lookup
- [ ] Add a focused internal helper layer that centralizes those lookups and protocol/mode checks without changing behavior yet
- [ ] Update `tcp.rs` and `udp.rs` to use the helper layer instead of open-coded mode matching
- [ ] Run focused host port-forward verification
- [ ] Commit with real changes only

### Task 2: Collapse Duplicate Listen-Session Ownership

**Intent:** Make the listen-session association a single tunnel-owned concern so `TunnelMode` no longer carries heavy session state and the helper layer can provide the one supported access path.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/types.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Preserve connect-tunnel behavior: connect mode must still carry no session and must continue to use connection-local TCP/UDP maps.
- Preserve listen-tunnel resume semantics, including retained listener/UDP-bind reactivation and session-expiry scheduling.
- Do not widen this task into the older `send_tunnel_error*` duplication or a broader sender-trait redesign unless execution proves a tiny helper is required to complete the ownership change safely.

**Verification:**
- Run: `cargo test -p remote-exec-host port_forward::port_tunnel_tests`
- Expect: host listen/connect tunnel behavior still passes after the ownership change.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon RPC forwarding behavior remains unchanged.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding behavior against the Rust daemon still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker forwarding behavior against the C++ daemon still passes, confirming no public contract drift.

- [ ] Confirm the canonical home for the listen-session handle on `TunnelState` and update naming if the current field name becomes misleading
- [ ] Remove `Arc<SessionState>` from `TunnelMode::Listen` and route listen-side operations through the helper layer plus the tunnel-owned session handle
- [ ] Recheck attach, detach, close, and resume flows so the session lifecycle still matches the current retained-resource behavior
- [ ] Run focused host, daemon, and broker forwarding verification
- [ ] Commit with real changes only

### Task 3: Final Phase 3 Confirmatory Sweep

**Intent:** Reconfirm the verified Phase 3 boundaries, document which audit claims were stale or contract-conflicting, and finish with the Rust quality gate.

**Relevant files/components:**
- Likely inspect: `docs/CODE_AUDIT_ROUND5.md`
- Likely inspect: the host port-forward files touched by Tasks 1 and 2
- Likely inspect: `AGENTS.md`

**Notes / constraints:**
- Keep the final summary explicit about exclusions: item `#15` narrowed out, item `#16` rejected by current architecture contract, and item `#19` already stale.
- Re-run targeted searches so the completion notes distinguish "implemented" from "already fixed" and "intentionally not selected."
- If a smaller helper-only refactor proves safer than the full ownership collapse, document that precisely instead of claiming the broader audit recommendation landed unchanged.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the Rust workspace passes end-to-end after the Phase 3 changes.
- Run: `cargo fmt --all --check`
- Expect: formatting stays clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions after the ownership cleanup.

- [ ] Re-run targeted searches for the Phase 3 seams and confirm which audit items were live, stale, or intentionally excluded
- [ ] Run the full Rust quality gate for the cross-cutting host port-forward refactor
- [ ] Summarize item `#15` as narrowed out, item `#16` as contract-conflicting under the current architecture, and item `#19` as already stale
- [ ] Commit any sweep-only adjustments if needed; otherwise do not create an empty commit
