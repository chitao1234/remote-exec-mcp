# Code Quality Audit Round 2 Tier 3 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Tighten the still-worthwhile Rust-side Tier 3 encapsulation seams without reopening the broader broker transport split or the C++ session-boundary redesign work.

**Requirements:**
- Keep this plan Rust-only. Defer the `daemon_client.rs` transfer split (`3.2`) and the C++ `LiveSession` ownership redesign (`3.5`) to later dedicated refactor passes.
- Preserve the public MCP surface, broker-daemon wire format, and current port-forward behavior.
- Treat `TargetHandle` raw-call cleanup narrowly. Do not force every operation into the same `_checked` wrapper pattern when the current path intentionally owns extra session or transport logic.
- Preserve existing `RpcErrorCode` wire values. For port-forward host errors, only the HTTP status classification and logging shape may change where the current `bad request` alias is incorrect.
- Keep commits medium-sized and reviewable. Avoid mixing the error-classification pass with unrelated broker/daemon-client movement.
- Do not create an empty sweep commit.

**Architecture:** The actionable Tier 3 work falls into three coherent Rust slices. First, close the two small broker-local encapsulation leaks by moving active-stream reservation behind `ForwardRuntime` and narrowing `BrokerState` field visibility to crate-internal use. Second, narrow `TargetHandle`’s raw dispatch boundary so the public type no longer exposes transport-level methods unnecessarily, while preserving intentionally special raw flows like `write_stdin`, transfer path probing, and port-tunnel setup where wrapper semantics differ. Third, replace the port-forward `logged_bad_request` alias with explicit host RPC error construction so client-validation errors stay `400` while operational connect/read/write/accept failures move to the appropriate server-side classification.

**Verification Strategy:** Use focused broker tests after each broker-facing task and focused daemon/broker port-forward tests after the host error-classification pass. Finish with the Rust workspace quality gate because this plan changes crate-internal visibility and error handling across both broker and daemon-host code.

**Assumptions / Open Questions:**
- Narrowing `BrokerState` fields and `TargetHandle` raw methods to `pub(crate)` is expected to be safe for the current workspace, but execution should confirm there is no in-workspace consumer outside the broker crate that depends on the wider visibility.
- `TargetHandle` checked wrappers are intentionally incomplete today; execution should preserve that distinction instead of inventing uniform wrappers for `exec_write`, transfer import/export, or `port_tunnel` unless a concrete simplification emerges.
- The port-forward error pass should confirm a consistent status policy for infrastructure failures before editing call sites. The plan assumes these failures should no longer be emitted as logged `400` bad requests.

**Planning-Time Verification Summary:**
- `3.1`: valid and in scope. `crates/remote-exec-host/src/port_forward/error.rs` still aliases `logged_bad_request` as `rpc_error`, and the alias is still used across operational TCP/UDP/tunnel failure paths.
- `3.3`: partially valid and in scope in narrowed form. `TargetHandle` raw transport methods remain `pub`, but only some operations have `_checked` wrappers today, and some raw call sites are intentional.
- `3.4`: valid and in scope. `try_reserve_active_tcp_stream` in `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs` still reaches through `runtime.store` directly instead of using a `ForwardRuntime` helper.
- `3.6`: valid and in scope. `BrokerState` fields in `crates/remote-exec-broker/src/state.rs` remain `pub` even though the meaningful usage is crate-internal.
- Out of scope by revalidation: `3.2` remains a real broker-internal refactor but is broader than this plan. `3.5` remains a real C++ ownership redesign and should not be mixed into this Rust pass.

---

### Task 1: Close The Small Broker Encapsulation Leaks

**Intent:** Cover the low-risk Tier `3.4` and `3.6` items by moving active-stream reservation behind `ForwardRuntime` and tightening `BrokerState` field visibility.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/state.rs`
- Likely inspect: `crates/remote-exec-broker/src/lib.rs`
- Likely inspect or modify: broker unit tests that directly construct `BrokerState`

**Notes / constraints:**
- Keep the `try_reserve_active_tcp_stream` behavior unchanged. This is an ownership cleanup, not another TCP bridge redesign.
- Do not remove the public `BrokerState` type re-export in this pass unless execution confirms it is both unused and unnecessary. The primary goal here is field visibility, not public type churn.
- If a field truly needs to remain wider than `pub(crate)` for a verified in-workspace use, keep that exception explicit rather than forcing a blanket visibility rule.

**Verification:**
- Run: `cargo test -p remote-exec-broker port_forward::tcp_bridge::tests -- --nocapture`
- Expect: TCP forward accounting and teardown behavior remain green after the `ForwardRuntime` helper move.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker-visible port-forward behavior still passes after the visibility and helper cleanup.

- [ ] Reconfirm the exact `runtime.store` reach-through and the current direct `BrokerState` field consumers
- [ ] Move active-stream reservation behind a `ForwardRuntime` helper without changing behavior
- [ ] Narrow `BrokerState` field visibility as far as current in-workspace consumers allow
- [ ] Update any directly affected unit tests or constructors
- [ ] Run focused broker verification
- [ ] Commit

### Task 2: Narrow `TargetHandle` Raw Dispatch To Intentional Crate-Internal Use

**Intent:** Cover the worthwhile part of Tier `3.3` by reducing unnecessary public raw transport entrypoints while preserving intentionally special raw flows.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/target/handle.rs`
- Likely modify: `crates/remote-exec-broker/src/target/capabilities.rs`
- Likely inspect or modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Likely inspect or modify: `crates/remote-exec-broker/src/tools/patch.rs`
- Likely inspect or modify: `crates/remote-exec-broker/src/tools/image.rs`
- Likely inspect or modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Likely inspect or modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Likely inspect or modify: `crates/remote-exec-broker/src/port_forward/side.rs`

**Notes / constraints:**
- Do not force `exec_write`, transfer import/export, transfer path-info, or `port_tunnel` into the same `_checked` wrapper pattern unless execution finds a small, clearly correct consolidation.
- Preserve the existing `write_stdin` daemon-restart and session invalidation logic in `tools/exec.rs`; the current raw `exec_write` path there is intentional.
- The target of this task is public API narrowing on the Rust type, not a behavioral rework of tool routing.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: exec and `write_stdin` behavior, including session handling and restart detection, remain unchanged.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: patch and image tool paths still use the correct checked/validated behavior.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: transfer path-info and remote transfer flows still compile and behave correctly after visibility tightening.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: port-tunnel setup paths still work with the narrowed method visibility.

- [ ] Reconfirm which raw `TargetHandle` methods are intentionally special-case paths versus ordinary checked tool calls
- [ ] Narrow raw method visibility to crate-internal use where possible without changing behavior
- [ ] Keep or adjust checked wrappers only where they already express a real shared seam
- [ ] Update any affected tool call sites or helper modules to match the new visibility
- [ ] Run focused broker verification
- [ ] Commit

### Task 3: Reclassify Host Port-Forward Operational Errors Away From `bad request`

**Intent:** Fix Tier `3.1` by replacing the misleading `logged_bad_request` alias with explicit host RPC error construction in the port-forward runtime.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/error.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely inspect or modify: `crates/remote-exec-host/src/port_forward/active.rs`
- Likely inspect or modify: `crates/remote-exec-host/src/port_forward/session_store.rs`
- Likely inspect or modify: `crates/remote-exec-host/src/port_forward/limiter.rs`
- Likely inspect or modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Likely inspect or modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Notes / constraints:**
- Do not blanket-convert every port-forward `rpc_error(...)` call to a server error. Keep true client-validation failures such as malformed tunnel input or invalid endpoint syntax on the request-rejection path.
- Preserve wire error codes and human-facing messages unless the current wording is tightly coupled to the wrong classification.
- If execution finds that some existing tests assert `400` specifically for operational failures, update them only where the new status policy is clearly the intended contract.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon-visible port-forward RPC behavior stays green with the corrected HTTP status classification.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker port-forward behavior remains green after the host runtime status changes.

- [ ] Reconfirm the current alias call sites and separate client-validation errors from operational infrastructure failures
- [ ] Replace the alias with explicit host RPC error construction and apply the agreed status policy per failure class
- [ ] Update focused daemon and broker tests only where the corrected classification changes asserted behavior
- [ ] Run focused port-forward verification
- [ ] Commit

### Task 4: Final Tier 3 Sweep And Rust Quality Gate

**Intent:** Confirm the final tree matches the reduced Tier 3 scope, explicitly leaves the broader refactors deferred, and finishes with the required Rust quality gate.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit-round-2.md`
- Likely inspect: the touched broker and host files from Tasks 1 through 3
- Existing references: `docs/superpowers/plans/2026-05-15-code-quality-audit-round-2-tier-6-remnant-plan.md`

**Notes / constraints:**
- Keep this sweep limited to the verified Tier 3 items from this plan. Do not expand into Tier 4+ work.
- Explicitly preserve the decision to defer the broader `daemon_client.rs` split and C++ `LiveSession` refactor.
- Do not create an empty sweep commit. Only commit here if the final verification pass forces a real follow-up change.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the Rust workspace remains green after the Tier 3 cleanup bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no new lint regressions are introduced.

- [ ] Re-run the Tier 3 verification queries and confirm the final code shape is intentional for each in-scope seam
- [ ] Reconfirm that `3.2` and `3.5` remain deferred by design rather than accidentally skipped
- [ ] Run the required Rust workspace quality gate
- [ ] Summarize which Tier 3 items were fixed and which broader Tier 3 refactors were intentionally left for later
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
