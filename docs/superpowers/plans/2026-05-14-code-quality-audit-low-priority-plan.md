# Code Quality Audit Low-Priority Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-worthwhile `9.*` items from `docs/code-quality-audit.md` with small-scope Rust cleanups, while explicitly deferring the low-value churn items that do not justify structural refactoring.

**Requirements:**
- Cover every `9.*` claim with an explicit disposition: implement, narrow, or defer.
- Keep the work Rust-only unless implementation uncovers a direct shared-code dependency that must move in lockstep.
- Preserve broker config behavior, daemon-client RPC behavior, port-forward reconnect behavior, host exec session lifecycle behavior, and daemon test semantics.
- Do not change public tool arguments, result schemas, RPC routes, wire formats, or target metadata.
- Do not rewrite `docs/code-quality-audit.md`; it is an input artifact, not the live contract.
- Continue the established execution style: medium-sized tasks, focused verification after each task, and no empty commits.

**Architecture:** Treat Section `9.*` as two implementation batches plus a final confirmatory sweep. The first batch is owner-local runtime maintenance: remove the redundant broker workdir normalization call where current validated-config flow already makes it unnecessary, and document the deliberate session-store lock/recheck/touch sequence so the concurrency invariant is explicit instead of implicit. The second batch is test-support cleanup: introduce a small Unix-side `ExecStartRequest` helper seam and apply it across the repetitive daemon exec RPC tests without changing what those tests cover. Leave the broker daemon-client status-log generic and the TCP no-op reconnect callback untouched in this pass, because both are factually real but too weak to justify reshaping otherwise stable seams.

**Verification Strategy:** Run focused Rust verification after each task, centered on broker config/startup coverage and daemon exec/session behavior. Finish with the Rust quality gate relevant to touched code: `cargo test --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

**Assumptions / Open Questions:**
- `BrokerConfig::validate()` is currently reached through `into_validated()` and load/startup flows that already normalize paths first; implementation should recheck this before removing the extra normalization call.
- The `SessionStore` triple-lock pattern is intentional for correctness, so the target outcome is comment-level clarification unless code inspection shows a smaller safe simplification.
- The daemon exec test helper should stay lightweight and local to the existing test support module; this plan does not require introducing a full fluent builder API.
- `9.2` and `9.3` remain intentionally deferred unless implementation uncovers a clearer owner-local cleanup that does not change helper boundaries or reconnect semantics.

**Planning-Time Verification Summary:**
- `9.1`: valid and in scope. [config.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/config.rs:303) normalizes `local.default_workdir`, and [config.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/config.rs:317) recomputes `normalized_default_workdir()` again during validation.
- `9.2`: partially valid but deferred. [daemon_client.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/daemon_client.rs:612) takes a generic status-log callback, but the callers attach different fields and messages, so removing the generic cleanly would first require a stronger shared log-context shape.
- `9.3`: valid but deferred. [tcp_bridge.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:41) passes `|| async {}` into [reconnect.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs:132), but the hook is there for UDP-specific dropped-datagram accounting and does not currently justify a broader reconnect-helper split.
- `9.4`: valid and in scope as documentation/clarity work. [store.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/exec/store.rs:148) intentionally locks the session, rechecks store ownership, then touches `last_touched_at`; the behavior is correct but under-explained.
- `9.5`: valid and in scope. [unix.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon/tests/exec_rpc/unix.rs:7) and many later tests inline the same `ExecStartRequest` boilerplate, and the existing helpers in [mod.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon/tests/exec_rpc/mod.rs:59) are Windows-only.

---

### Task 1: Tighten Broker Validation And Document Host Session Invariants

**Intent:** Remove the currently redundant broker config workdir normalization step and make the host exec session-store correctness pattern explicit for future maintainers.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely modify: `crates/remote-exec-host/src/exec/store.rs`
- Existing references: `crates/remote-exec-host/src/config/mod.rs`
- Existing references: broker startup/config load paths that call `BrokerConfig::into_validated()`

**Notes / constraints:**
- Cover audit items `9.1` and `9.4`.
- Reconfirm during execution that no meaningful call path depends on `BrokerConfig::validate()` performing normalization on an unnormalized `local.default_workdir`.
- Prefer using the already-normalized stored field in broker validation instead of adding another helper layer.
- For the session store, aim for an explanatory comment or similarly small clarity improvement; do not restructure the locking algorithm unless a safe simplification is obvious and behavior-preserving.
- Preserve existing error messages, validation boundaries, and session timeout semantics.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: broker startup/config-driven behavior still passes after the validation cleanup.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: host-backed exec session behavior still passes after the session-store documentation cleanup.

- [ ] Reconfirm the validated-config call flow and the exact `SessionStore` double-check invariant in current code
- [ ] Update broker validation to rely on the normalized stored workdir field where safe
- [ ] Add targeted clarity around the session-store lock/recheck/touch sequence without changing behavior
- [ ] Run focused broker and daemon verification for config/startup and exec-session coverage
- [ ] Commit with real changes only

### Task 2: Consolidate Unix Exec RPC Test Request Setup

**Intent:** Reduce repetitive `ExecStartRequest` literals in the daemon Unix exec RPC tests by introducing a small shared helper seam that preserves test readability and current assertions.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/tests/exec_rpc/mod.rs`
- Likely modify: `crates/remote-exec-daemon/tests/exec_rpc/unix.rs`
- Possibly modify: `crates/remote-exec-daemon/tests/windows_pty_debug.rs` if the new helper naturally applies there without obscuring the test-specific shell choice

**Notes / constraints:**
- Cover audit item `9.5`.
- Keep the helper lightweight and explicit. A small constructor-style helper is preferred; a fluent builder API is optional and should only be introduced if it clearly improves readability.
- Preserve existing per-test overrides such as `tty`, `yield_time_ms`, `max_output_tokens`, `login`, and shell selection.
- Do not fold Windows-specific shell helpers into the Unix helper path unless the shared API stays simpler than the current split.
- Avoid sweeping churn across unrelated tests; stop once the repeated Unix boilerplate is materially reduced and the helper boundary is clear.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: daemon exec RPC coverage still passes with unchanged behavioral assertions.

- [ ] Reconfirm the repeated Unix-side `ExecStartRequest` shapes and identify the smallest helpful shared constructor
- [ ] Add the shared helper in `exec_rpc` test support and adopt it through the repetitive Unix tests
- [ ] Apply the same helper to any immediately adjacent test file only if it clearly reduces duplication without hiding intent
- [ ] Run focused daemon exec RPC verification
- [ ] Commit with real changes only

### Task 3: Final `9.*` Sweep And Rust Quality Gate

**Intent:** Confirm that the low-priority plan removed the intended live seams, left the deferred items intentionally deferred, and did not introduce regressions.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: `crates/remote-exec-broker/src/config.rs`
- Likely inspect: `crates/remote-exec-host/src/exec/store.rs`
- Likely inspect: `crates/remote-exec-daemon/tests/exec_rpc/`

**Notes / constraints:**
- Keep this task confirmatory. Do not widen into unrelated sections of `docs/code-quality-audit.md`.
- Re-run targeted searches to confirm `9.1` and `9.5` are materially reduced and `9.4` is now explicitly documented.
- If `9.2` or `9.3` still look tempting during the sweep, record them as intentionally deferred rather than opportunistically widening the change.

**Verification:**
- Run: `cargo test --workspace`
- Expect: full Rust workspace passes.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions.

- [ ] Re-run searches/diffs against the planned `9.*` seams and confirm the final scope stayed disciplined
- [ ] Run the Rust workspace quality gate
- [ ] Summarize which `9.*` items were fixed versus intentionally deferred
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
