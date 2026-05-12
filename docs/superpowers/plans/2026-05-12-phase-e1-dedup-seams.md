# Phase E1 Dedup Seams Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Land the first Phase E1 maintainability slice by removing the broker-local `"local"` routing holdout and consolidating shared port-tunnel metadata plus raw metadata codec helpers into `remote-exec-proto` without changing public behavior.

**Requirements:**
- Cover the approved mixed first batch: audit items `#9`, `#15`, and the minimal adjacent seam from `#16`.
- Keep behavior stable for broker target routing, port-forward wire format, and host/broker error handling.
- Keep broker-owned local routing semantics intact: `target: "local"` remains the public/runtime name.
- Do not widen this batch into broker-local control-flow enum cleanup, queue-accounting cleanup, or C++ daemon changes.
- Preserve existing public broker tests and daemon RPC tests as the primary regression guard.

**Architecture:** `remote-exec-proto` becomes the canonical home for port-tunnel wire metadata DTOs and raw JSON metadata serialization helpers because those are contract-level concerns shared by broker and host. `remote-exec-broker` keeps broker-local routing and state helpers, but centralizes the broker-host local target name behind a single constant for runtime code paths. `remote-exec-host` and `remote-exec-broker` continue to own their own error adaptation, wrapping proto metadata encode/decode failures at their boundaries instead of leaking crate-specific error types into the protocol crate.

**Verification Strategy:** Use focused integration coverage that already exercises the affected seams: `cargo test -p remote-exec-daemon --test port_forward_rpc` and `cargo test -p remote-exec-broker --test mcp_forward_ports`. Use targeted search checks to confirm broker runtime `"local"` holdouts are replaced where intended and that duplicate tunnel metadata structs/helpers are removed from broker/host implementation files.

**Assumptions / Open Questions:**
- Keep user-facing examples, fixtures, and public schema literals that intentionally spell `"local"` unless the touched code path naturally benefits from the new constant.
- Confirm during execution whether `state.rs` is still the best long-term home for `LOCAL_TARGET_NAME`; if another existing broker-local constants module appears more coherent, move the constant there without widening scope.
- Keep broker-local `events::TunnelRole` unchanged in this batch unless a call site proves impossible to update cleanly without it.

---

### Task 1: Save The Phase E1 Plan

**Intent:** Create the tracked plan artifact for the approved first E1 slice before implementation starts.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-12-phase-e1-dedup-seams.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start code edits for the implementation slice until the plan is reviewed and approved.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-12-phase-e1-dedup-seams.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged design + execution plan at the tracked path
- [ ] Check the plan header, goal, and scope against the approved design
- [ ] Confirm the plan stays limited to the first E1 batch
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: Centralize The Broker Local Target Name

**Intent:** Remove the broker runtime maintenance hazard around the `"local"` magic string by introducing one canonical constant and using it in broker-owned routing paths.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/state.rs`
- Likely modify: `crates/remote-exec-broker/src/startup.rs`
- Likely modify: `crates/remote-exec-broker/src/local_port_backend.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/side.rs`

**Notes / constraints:**
- Focus on broker runtime/config code paths where typos would silently break local routing semantics.
- Do not churn every test fixture or documentation example that uses the `"local"` string unless it is part of a touched runtime helper.
- Preserve the current behavior where forwarding can use broker-host local even when no explicit local target exists in configured targets.

**Verification:**
- Run: `rg -n '\"local\"' crates/remote-exec-broker/src`
- Expect: remaining hits are either intentional user-facing literals, tests, or untouched code outside the scoped runtime holdouts.

- [ ] Inspect the current broker runtime holdouts and decide the narrow replacement set
- [ ] Add `LOCAL_TARGET_NAME` in the chosen broker-local home and update imports
- [ ] Replace the scoped runtime/config holdouts to use the constant
- [ ] Re-run targeted search to confirm the intended holdouts are gone
- [ ] Commit

### Task 3: Move Shared Tunnel Metadata And Raw Codec Helpers Into Proto

**Intent:** Make `remote-exec-proto` the single source of truth for shared port-tunnel metadata DTOs and raw metadata serialization helpers used by both broker and host.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/port_tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/types.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/codec.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`

**Notes / constraints:**
- Move only the shared wire DTOs identified in the approved batch: `EndpointMeta`, `TcpAcceptMeta`, and `UdpDatagramMeta`.
- Add raw `encode_*` and `decode_*` helpers in proto for metadata bytes, but keep host/broker error wrapping local to those crates.
- Keep any host-only or broker-only metadata structs local if they are not actually shared wire contract types.
- Do not change the tunnel frame JSON shape or the semantics of metadata validation errors.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon-side port-forward RPC coverage still passes with unchanged wire behavior.

- [ ] Confirm the exact shared DTO set and any host-only metadata that should remain local
- [ ] Add canonical metadata DTOs and raw metadata codec helpers to `remote-exec-proto`
- [ ] Remove duplicate DTO/helper definitions from broker and host, updating imports and local error adaptation
- [ ] Rebuild the touched port-forward call sites around the new proto helpers
- [ ] Run focused daemon RPC verification
- [ ] Commit

### Task 4: Run Broker Forwarding Regression Coverage

**Intent:** Verify the combined `#9`, `#15`, and `#16` slice against the broker’s public forwarding surface after the refactor lands.

**Relevant files/components:**
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`

**Notes / constraints:**
- Keep verification targeted first; only widen if focused tests expose an integration seam not covered by the planned commands.
- If the broker test reveals behavior drift, fix the regression in the same slice instead of masking it with test updates unless the old expectation was clearly wrong.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker public forwarding behavior remains unchanged for the Rust-backed forwarding path.

- [ ] Run focused broker forwarding verification
- [ ] Review failures for behavioral drift versus refactor fallout
- [ ] Fix any scoped regressions in the touched seams
- [ ] Re-run focused verification until green
- [ ] Commit
