# Code Quality Audit Round 2 Tier 2 Remnant Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Finish the only still-worthwhile Tier 2 cleanup by hardening broker TCP active-stream accounting without reopening already-fixed audit items or changing public port-forward behavior.

**Requirements:**
- Preserve the public MCP surface, broker-daemon wire format, and current v4 port-forward behavior.
- Treat audit items `2.1` through `2.9` as already fixed or no longer applicable to the current code shape; do not reopen them unless execution uncovers a fresh live bug in the current tree.
- Keep the scope local to broker-side TCP forward internals. Do not turn this into another port-forward redesign pass.
- Improve the `active_tcp_streams` accounting seam only if the result makes double-release or missed-release paths harder to introduce in future edits.
- Preserve existing dropped-stream telemetry semantics. Do not blur `dropped_tcp_streams` updates together with ordinary close/release paths unless the current behavior already treats them as the same case.

**Architecture:** The current broker TCP bridge already centralizes the actual counter mutation on `ForwardRuntime`, but `tcp_bridge.rs` still spreads the decision about when to release or drop an active stream across many branches. The intended cleanup is to funnel stream teardown through a narrower internal accounting shape so a removed active stream settles exactly once. This should stay inside `tcp_bridge.rs` and, if useful, a tiny helper addition on `ForwardRuntime` in `supervisor.rs`; it should not introduce a broad new abstraction layer or alter the forward state machine.

**Verification Strategy:** Use focused broker verification for the touched area first, especially the internal TCP bridge tests and public broker port-forward coverage. Finish with a broker lint pass so the cleanup does not leave dead helper shapes or warning-only scaffolding behind.

**Assumptions / Open Questions:**
- The old Tier 2 plan is obsolete for current execution and should not be reused as-is.
- Execution should confirm whether the cleanest shape is a tiny local disposition helper in `tcp_bridge.rs` or one additional `ForwardRuntime` helper; a cross-`await` RAII guard is only acceptable if it stays borrow-safe and does not obscure the control flow.
- The existing `tcp_bridge.rs` tests that assert `active_tcp_streams == 0` after failures are expected to remain the main regression net; execution should add or tighten tests only where the current coverage is insufficient for the chosen helper shape.

---

### Task 1: Collapse TCP Active-Stream Settlement To One Explicit Internal Shape

**Intent:** Remove the remaining fragile scatter in broker TCP active-stream teardown without changing forward behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Existing references: broker TCP bridge tests in `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`

**Notes / constraints:**
- Keep the cleanup local to active-stream settlement. Do not refactor unrelated tunnel event handling while touching the file.
- Preserve the current distinction between ordinary release, dropped-stream release, and bulk reconnect cleanup.
- Prefer an explicit helper or disposition flow over clever lifetime tricks if async borrow boundaries make a scope guard awkward.

**Verification:**
- Run: `cargo test -p remote-exec-broker`
- Expect: broker unit and integration tests, including TCP forward accounting paths, remain green.

- [ ] Inspect every path that reserves, drops, bulk-releases, or ordinarily releases an active TCP stream and confirm the current invariants
- [ ] Introduce the narrowest internal helper shape that makes each removed active stream settle exactly once
- [ ] Tighten or add focused tests only where the chosen helper shape needs a clearer regression net
- [ ] Run focused broker verification
- [ ] Commit

### Task 2: Revalidate The Tier 2 Close-Out After The Cleanup

**Intent:** Confirm the small cleanup did not accidentally reopen stale Tier 2 issues and that the final tree still matches the reduced Tier 2 scope.

**Relevant files/components:**
- Likely inspect: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely inspect: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Existing references: `docs/code-quality-audit-round-2.md`

**Notes / constraints:**
- This is a revalidation task, not permission to broaden scope back to the original audit document.
- Only make a follow-up code change if verification uncovers a real regression from Task 1.

**Verification:**
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions from the accounting cleanup.
- Run: `cargo test -p remote-exec-broker`
- Expect: the broker crate stays green after the final tree settles.

- [ ] Recheck the final `tcp_bridge.rs` accounting shape against the original Tier 2 concern and confirm the remaining cleanup value was actually addressed
- [ ] Reconfirm that Tier 2 items `2.1` through `2.9` remain fixed or out-of-scope for the current tree
- [ ] Run the final verification commands
- [ ] Commit any real follow-up change only if verification uncovers one; do not create an empty sweep commit
