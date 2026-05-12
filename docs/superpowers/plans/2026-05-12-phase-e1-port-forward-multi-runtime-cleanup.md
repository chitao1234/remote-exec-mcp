# Phase E1 Port-Forward Multi-Runtime Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Land the next larger Phase E1 maintainability slice by removing the approved duplicated port-forward seams across broker Rust, host Rust, shared proto, and the C++ daemon without changing the public forwarding contract.

**Requirements:**
- Cover the approved mixed bundle: audit items `#12`, `#13`, `#14`, `#17`, `#18`, and `#19`.
- Keep public `forward_ports` behavior, broker-owned IDs, v4 tunnel protocol semantics, and current target metadata unchanged.
- Include both Rust and C++ work in this batch, but keep each refactor at the narrowest owner boundary.
- Do not widen this slice into unrelated E1 items such as logging/PKI cleanup, reconnect policy redesign, or new abstraction layers that cut across broker/host/C++ boundaries.
- Continue the user’s plan-based execution preference and commit after each task only when that task has real code changes; do not create empty commits.

**Architecture:** Keep test-only broker harness code broker-local, keep host stream lifecycle cleanup inside `remote-exec-host`, and keep C++ `TunnelReady` construction file-local to the daemon transport implementation. Use shared proto ownership only for contract-level concepts that already cross module boundaries, specifically the canonical `TunnelRole` enum and any frame-level "is this data-plane traffic?" helper that can be expressed without leaking runtime-specific state. Broker tunnel-open orchestration should collapse to one supervisor helper that preserves the current listen/connect-specific validation while removing the duplicated send/wait/match handshake path.

**Verification Strategy:** Verify each task with the narrowest existing coverage first. For broker-only refactors, run focused broker unit/integration coverage around the touched bridge or supervisor paths. For host/proto forwarding-runtime refactors, run `cargo test -p remote-exec-daemon --test port_forward_rpc` and `cargo test -p remote-exec-broker --test mcp_forward_ports`. For C++ transport refactors, run `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`, widening to `make -C crates/remote-exec-daemon-cpp check-posix` only if the helper extraction touches broader build surfaces.

**Assumptions / Open Questions:**
- The broker bridge harness duplication should move into a `#[cfg(test)]` helper module under `crates/remote-exec-broker/src/port_forward/` rather than a new workspace-level test utility.
- `TunnelRole` should have one canonical definition in `remote-exec-proto`; if broker event handling still wants a local import surface, prefer re-exporting the proto enum over keeping a second broker-only enum.
- The duplicated broker/host charge predicates are only safe to unify if the shared helper preserves the current queue-accounting behavior for empty control frames versus data-plane frames.
- The C++ `TunnelReady` dedup should remain a small file-local helper unless implementation reveals an existing nearby utility that already owns tunnel-ready metadata assembly.

---

### Task 1: Save The Multi-Runtime Phase E1 Plan

**Intent:** Create the tracked plan artifact for the approved larger E1 port-forward cleanup batch before implementation starts.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-12-phase-e1-port-forward-multi-runtime-cleanup.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start implementation for this batch until the plan is reviewed and approved.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-12-phase-e1-port-forward-multi-runtime-cleanup.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged design + execution plan at the tracked path
- [ ] Check the header, scope, and task breakdown against the approved six-item bundle
- [ ] Confirm the plan excludes unrelated E1 items
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: Deduplicate The Broker Port-Forward Test Harness

**Intent:** Remove the byte-for-byte duplicated scripted tunnel test harness shared by `tcp_bridge.rs` and `udp_bridge.rs` while keeping the tests readable and behaviorally identical.

**Relevant files/components:**
- Likely create: `crates/remote-exec-broker/src/port_forward/test_support.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`

**Notes / constraints:**
- Keep the shared harness test-only; do not introduce new production dependencies or exports for this cleanup.
- Consolidate only the identical helpers called out by the audit (`AsyncRead`/`AsyncWrite` harness state, `fail_writes`, `push_read_frame`, `wait_for_written_frame`, `pop_matching_written_frame`, `wait_until_send_fails`, `filter_one`, `test_record`) unless execution shows a nearby helper must move with them to keep the module coherent.
- Preserve current test readability in both files; the goal is less duplication, not indirect test code that obscures intent.

**Verification:**
- Run: `cargo test -p remote-exec-broker tcp_bridge`
- Expect: the TCP bridge unit tests compile and pass with the shared harness.
- Run: `cargo test -p remote-exec-broker udp_bridge`
- Expect: the UDP bridge unit tests compile and pass with the shared harness.

- [ ] Inspect the duplicated broker test harness blocks and confirm the exact shared surface
- [ ] Add one broker-local `#[cfg(test)]` helper module for the shared harness pieces
- [ ] Update `tcp_bridge.rs` and `udp_bridge.rs` to use the shared helper without changing test intent
- [ ] Run focused broker bridge verification
- [ ] Commit with real code changes only

### Task 3: Deduplicate Rust Port-Forward Runtime Seams Across Host, Broker, And Proto

**Intent:** Remove the remaining approved Rust forwarding duplication by collapsing identical stream-cleanup helpers, tunnel-open handshake logic, broker-local `TunnelRole`, and the duplicate data-plane charge predicate into their narrowest shared owners.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/limiter.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/events.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-proto/src/port_tunnel.rs`

**Notes / constraints:**
- Keep host stream cleanup local to `tcp.rs`; a small generic helper or local adapter is preferred over a new trait or shared runtime abstraction.
- Keep listen/connect-specific validation differences in `supervisor.rs` even if the send/wait/match handshake becomes one helper.
- Make `remote-exec-proto` the single source of truth for `TunnelRole`; broker event code can import or re-export it, but should not keep a duplicate enum.
- Only share the queue-charge predicate if it can be expressed in terms of the existing tunnel `Frame` contract and keeps current queued-byte semantics unchanged on both broker and host.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon-side forwarding RPC behavior still passes with unchanged tunnel/runtime semantics.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker public forwarding behavior still passes with the refactored handshake and queue-accounting helpers.

- [ ] Confirm the duplicated cleanup, handshake, enum, and frame-charge seams still match the approved scope
- [ ] Refactor host TCP cleanup/cancel helpers to one local pattern without changing lifecycle semantics
- [ ] Collapse the broker tunnel-open handshake to one helper that preserves listen/connect-specific checks
- [ ] Remove the duplicate broker `TunnelRole` in favor of the canonical proto enum and align call sites
- [ ] Share the frame-level data-plane charge predicate only if current accounting behavior remains identical
- [ ] Run focused daemon and broker forwarding verification
- [ ] Commit with real code changes only

### Task 4: Deduplicate C++ `TunnelReady` Construction

**Intent:** Remove the duplicated `TunnelReady` JSON assembly in the C++ daemon transport while preserving the current listen/connect response shape exactly.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/include/port_tunnel*.h` if a declaration becomes necessary during extraction

**Notes / constraints:**
- Prefer a file-local helper that accepts the role-specific optional fields (`session_id`, `resume_timeout_ms`) while reusing the identical `limits` object construction.
- Do not change the serialized field names, optional-field behavior, or listen/connect branching semantics.
- If extraction stays entirely within the `.cpp` file, avoid widening the header surface.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: the C++ host/server streaming coverage still passes with unchanged tunnel-ready behavior.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the broader POSIX build/test gate still passes if the helper extraction touched shared compile paths.

- [ ] Confirm the duplicated `TunnelReady` construction remains limited to the listen/connect branches
- [ ] Extract one file-local helper for ready-meta assembly and update both branches to use it
- [ ] Keep the serialized JSON shape identical for both roles
- [ ] Run focused C++ verification, widening to `check-posix` only if needed
- [ ] Commit with real code changes only
