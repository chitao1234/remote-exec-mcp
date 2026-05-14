# Code Quality Audit Round 2 Tier 6 Remnant Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Close out the only still-worthwhile Tier 6 cleanup by removing the last genuine single-use helper noise in host exec handling without reopening the stale Tier 6 audit claims.

**Requirements:**
- Keep Tier 6 scope narrow to the one still-actionable helper seam in `crates/remote-exec-host/src/exec/handlers.rs`.
- Treat the rest of Tier 6 as already solved, stale, or not worth churn in the current tree. Do not reopen those items unless execution uncovers a fresh correctness or layering issue.
- Preserve daemon exec behavior, warning text, response shape, and all broker-visible exec semantics.
- Do not turn this cleanup into a larger host exec refactor. The result should be behavior-neutral.
- Do not create an empty sweep commit.

**Architecture:** The current Tier 6 remnant is `session_limit_warnings`, a tiny helper that is still called from only one site in `store_running_session`. The intended cleanup is to inline that warning construction at the call site, leaving the surrounding `running_session_response` bridge in place because it now has multiple callers and still expresses a real seam. The rest of the old Tier 6 plan should be treated as historical context rather than execution scope.

**Verification Strategy:** Use focused exec RPC coverage first because the touched code sits in host exec response assembly. Finish with a lint pass so the helper removal does not leave dead code or warning-only churn behind.

**Assumptions / Open Questions:**
- `session_limit_warnings` remains single-use at execution time; if a second live caller appears before editing, keep the helper and stop rather than forcing churn.
- The warning text for session-limit threshold responses is already covered well enough by existing daemon exec RPC tests; execution should add coverage only if the current tests do not exercise the inlined path.
- The older `docs/superpowers/plans/2026-05-14-code-quality-audit-round-2-tier-5-6-plan.md` artifact is obsolete for Tier 6 execution and should not be reused as-is.

**Planning-Time Verification Summary:**
- In scope: `session_limit_warnings` in `crates/remote-exec-host/src/exec/handlers.rs` is still a true single-use helper.
- Out of scope by revalidation: `running_session_response` now has multiple callers; `invalid_enum_header` and `apply_daemon_client_timeouts` are reused helpers; `TcpReadLoopTarget` is already gone in favor of `TcpReadLoopContext`; the C++ import overload cascade is already collapsed; and the older broker exec helper claims from Tier 6 no longer match the current tree.

---

### Task 1: Inline The Last Single-Use Host Exec Warning Helper

**Intent:** Remove the only still-live Tier 6 single-use helper without changing host exec behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Likely inspect: `crates/remote-exec-daemon/tests/exec_rpc/`
- Existing references: host exec response helpers in `crates/remote-exec-host/src/exec/response.rs`

**Notes / constraints:**
- Keep `running_session_response` unless execution finds that it has become single-use again, which is not expected from planning-time verification.
- Preserve `ExecWarning::session_limit_approaching(...)` text and threshold behavior exactly.
- If the warning creation reads more clearly as a tiny local block than as a nested expression, prefer the clearer local block rather than golfing the code.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: exec start/write behavior and warning propagation remain green after the helper is inlined.

- [ ] Reconfirm that `session_limit_warnings` still has exactly one live caller and that no second seam appeared since planning
- [ ] Inline the warning creation into `store_running_session` and remove the dead helper
- [ ] Add or tighten test coverage only if the current exec RPC tests do not exercise the warning path clearly enough
- [ ] Run focused exec verification
- [ ] Commit

### Task 2: Revalidate The Tier 6 Close-Out And Keep The Rest Narrow

**Intent:** Confirm the final tree still matches the reduced Tier 6 scope and that no stale Tier 6 claim was accidentally reopened while making the small cleanup.

**Relevant files/components:**
- Likely inspect: `crates/remote-exec-host/src/exec/handlers.rs`
- Likely inspect: `docs/code-quality-audit-round-2.md`
- Existing references: `docs/superpowers/plans/2026-05-14-code-quality-audit-round-2-tier-5-6-plan.md`

**Notes / constraints:**
- This is a revalidation task, not permission to revive the old combined Tier 5/6 execution scope.
- Only make a follow-up code change if verification uncovers a real regression from Task 1.
- If verification remains clean, finish without another commit.

**Verification:**
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: the helper removal introduces no lint regressions.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: the daemon exec RPC surface remains green on the final tree.

- [ ] Recheck the final `handlers.rs` shape against the reduced Tier 6 goal and confirm the only worthwhile helper cleanup was actually removed
- [ ] Reconfirm that the other Tier 6 audit claims remain stale, already solved, or intentionally out of scope for the current tree
- [ ] Run the final verification commands
- [ ] Commit any real follow-up change only if verification uncovers one; do not create an empty sweep commit
