# Code Quality Audit Cpp Quality Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live C++ code-quality issues from section `6.*` of `docs/code-quality-audit.md` with medium-sized, low-risk cleanup batches, while explicitly deferring any test-framework migration.

**Requirements:**
- Verify every `6.*` audit claim against the current tree and only plan fixes for the claims that are still real.
- Preserve current public broker/daemon behavior, wire format, route payloads, and C++ daemon feature scope.
- Keep C++ changes within the current C++11 and XP-capable toolchain envelope; do not reintroduce the earlier C++03 compatibility framing.
- Prefer owner-local cleanup and clearer scope/lifetime boundaries over broad abstraction layers.
- Keep the existing C++ test harness style for now; `6.5` is not part of this plan unless explicitly reopened as a separate redesign.
- Continue the established execution style: medium-sized batches, focused verification per task, no worktrees, and no empty commits.
- Do not edit `docs/code-quality-audit.md`; it remains historical input, not the live contract.

**Architecture:** Treat the verified `6.*` issues as three implementation batches plus a final sweep. First, add small RAII and misuse-detection seams around environment mutation, filesystem iteration, and reference counters, because those are behavior-preserving safety improvements. Second, split the large `test_session_store.cpp` helpers into scenario-focused helpers that preserve coverage while improving failure isolation and reviewability. Third, narrow the route-test aggregation issue without forcing a framework migration: keep the existing `assert()` harness, but reduce helper breadth and make the top-level route coverage easier to reason about.

**Verification Strategy:** Run focused C++ host tests after each task, then finish with `make -C crates/remote-exec-daemon-cpp check-posix`. If Rust-facing broker tests are touched indirectly by route or runtime behavior, run the relevant focused broker tests for the affected surface before the final sweep.

**Assumptions / Open Questions:**
- `6.1` is fully valid in `test_session_store.cpp`, but the `test_server_routes_shared.cpp` portion is narrower than the audit wording suggests because the file is already split into several helpers; execution should improve isolation there without inventing unnecessary further splits.
- `6.4` needs a diagnostic policy decision during implementation: the safest likely outcome is a debug-time assertion or test-visible invariant check rather than a silent no-op or a behavior-changing runtime exception path.
- `6.5` is intentionally deferred in this plan. If improved failure messages or selective test execution become urgent, create a separate plan for a framework decision and build-system rollout.
- Any RAII helper added for tests should stay test-local unless the same helper shape is clearly reusable in production C++ code without expanding scope.

**Planning-Time Verification Summary:**
- `6.1`: valid and narrowed. `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp` still has two large multi-scenario helpers at `assert_stdin_and_tty_behavior(...)` and `assert_pruning_and_recency_behavior(...)`. `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp` is still fail-fast aggregate coverage, but it is already partially split into individual route helpers, so the problem there is aggregation breadth rather than one giant test body.
- `6.2`: valid and in scope. `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp` still manually saves and restores `PATH` and `TERM` with `setenv(...)` and `unsetenv(...)`, so any `assert(...)` failure in the middle can leak environment changes into later tests.
- `6.3`: valid and in scope. `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp` still manually owns `HANDLE` and `DIR*` lifetime across filesystem iteration.
- `6.4`: valid and in scope. `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp` still silently returns from `release_counter(...)` when the counter is already zero.
- `6.5`: valid but deferred. The test suite still uses raw `assert()`, but replacing the harness is a separate tooling decision rather than a cleanup-sized change.

---

### Task 1: Add RAII And Misuse Guards For The Live Safety Seams

**Intent:** Fix the still-live cleanup and misuse-detection issues from `6.2`, `6.3`, and `6.4` without changing wire behavior, route semantics, or the overall test harness model.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Likely inspect: `crates/remote-exec-daemon-cpp/include/port_tunnel*.h`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp` for existing test-helper style

**Notes / constraints:**
- Cover audit items `6.2`, `6.3`, and `6.4`.
- Prefer tiny owner-local RAII wrappers or guard structs over a new shared utility layer unless the same shape is clearly needed in more than one production file.
- Keep environment guards test-local unless there is a compelling reason to reuse them elsewhere.
- For `release_counter(...)`, favor a diagnostic that makes misuse visible during development and tests without introducing a new user-visible runtime contract.
- Do not bundle unrelated route or session-store test restructuring into this task.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- Expect: transfer filesystem iteration coverage still passes after the `DIR*` / `HANDLE` cleanup.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: runtime and port-tunnel-adjacent coverage still passes after the counter diagnostic change.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the touched C++ code compiles and the broader POSIX host suite stays green.

- [ ] Reconfirm the exact env-var, filesystem-iteration, and counter-release seams at the current code locations
- [ ] Add a test-local environment guard for `PATH` / `TERM` mutation in `test_session_store.cpp`
- [ ] Wrap `DIR*` and Windows find-handle ownership with small scope-bound cleanup helpers in `transfer_ops_fs.cpp`
- [ ] Make `release_counter(...)` diagnose double-release style misuse instead of silently returning
- [ ] Run focused transfer/runtime verification and then the POSIX C++ check
- [ ] Commit with real changes only

### Task 2: Split The Large Session-Store Scenario Helpers

**Intent:** Break the two oversized `test_session_store.cpp` helpers into scenario-focused helpers so failures are easier to localize and future maintenance does not require re-reading 200-line blocks.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Likely inspect: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely inspect: `crates/remote-exec-daemon-cpp/include/session_store.h`

**Notes / constraints:**
- Cover the main still-live portion of audit item `6.1`.
- Preserve the current scenario coverage:
  - non-tty stdin-closed rejection
  - tty round-trip behavior
  - unrelated-session write isolation
  - tty detection
  - tty resize success
  - non-tty resize rejection
  - pruning by limit
  - recency preservation
  - exited-session pruning
  - protected recent-session behavior
- Keep helper naming and local setup patterns readable; the goal is better isolation, not abstraction for its own sake.
- Maintain XP-specific coverage without forcing the POSIX and Windows flows into a fake common helper.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Expect: session-store host coverage still passes with the split helpers.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the refactored test file still compiles and passes in the broader POSIX suite.

- [ ] Reconfirm the current scenario grouping inside `assert_stdin_and_tty_behavior(...)` and `assert_pruning_and_recency_behavior(...)`
- [ ] Extract scenario-scoped helpers with clear names and minimal shared setup
- [ ] Preserve platform-specific behavior without over-generalizing the test code
- [ ] Keep the main driver readable and ensure all previous scenarios still execute
- [ ] Run focused session-store verification and then the POSIX C++ check
- [ ] Commit with real changes only

### Task 3: Narrow Route-Test Aggregation And Improve Failure Isolation

**Intent:** Clean up the narrower `test_server_routes_shared.cpp` portion of `6.1` by reducing helper breadth and making the top-level route test driver easier to extend and diagnose, without changing the underlying `assert()`-based harness.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.h`
- Likely inspect: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Likely inspect: `crates/remote-exec-daemon-cpp/tests/test_server_routes_common.cpp`

**Notes / constraints:**
- Cover the narrowed route-test portion of audit item `6.1`.
- Do not treat this task as a stealth migration to doctest/Catch2/another framework.
- Keep the existing route coverage, request construction, and response assertions intact unless a verified flaky or redundant check naturally falls out during the split.
- Favor smaller route scenario helpers and a clearer top-level execution order over introducing a second dispatch abstraction layer.
- If execution proves the current helper sizes are already acceptable, limit this task to the specific over-broad helpers rather than forcing churn everywhere.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: server-runtime and route coverage still passes after helper narrowing.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: adjacent HTTP route coverage still passes and no shared helper assumptions break.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the full touched POSIX C++ route suite remains green.

- [ ] Reconfirm which route helpers are still too broad versus already acceptably scoped
- [ ] Split only the over-broad route scenarios or top-level sequencing seams that materially improve failure isolation
- [ ] Keep the current route assertions and coverage stable while simplifying the execution flow
- [ ] Re-run focused route/runtime verification and then the POSIX C++ check
- [ ] Commit with real changes only

### Task 4: Final `6.*` Sweep And Explicit Defer Confirmation

**Intent:** Confirm the verified `6.*` issues were fixed or intentionally deferred, and record that `6.5` remains a separate harness-redesign decision rather than unfinished cleanup.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: the C++ files touched by Tasks 1 through 3

**Notes / constraints:**
- Keep this sweep limited to section `6.*`.
- Reconfirm that any remaining large helpers are intentionally scoped and not accidental leftovers.
- If `release_counter(...)` adopts a debug-only diagnostic, verify that the final behavior is explicit and documented in the implementation notes.
- Do not expand this sweep into dependency management, security, or other later audit sections.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the final combined C++ cleanup remains green.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker-to-C++ integration still passes if the touched runtime or route helpers affect shared behavior.

- [ ] Re-run searches for the verified `6.*` seams and confirm the remaining shape is intentional
- [ ] Run the required C++ POSIX quality gate
- [ ] Run the broker-to-C++ integration test if runtime-adjacent code changed in a way that could affect integration
- [ ] Summarize which `6.*` items were fixed, narrowed, or intentionally deferred
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
