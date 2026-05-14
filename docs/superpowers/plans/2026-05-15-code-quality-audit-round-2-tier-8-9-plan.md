# Code Quality Audit Round 2 Tier 8 and Tier 9 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Tier 8 and selected Tier 9 findings from `docs/code-quality-audit-round-2.md` with a bounded mix of test-structure cleanup and small production polish, without changing the public broker or daemon contracts.

**Requirements:**
- Preserve the public MCP tool surface, broker-daemon wire format, and v4 port-forward behavior.
- Keep C++ work within the repository's C++11 and XP-compatible toolchain expectations. Do not add third-party test frameworks or raise the language level.
- Treat Tier 8.1 as a real test-infrastructure change: touched C++ tests must no longer depend on `assert()` disappearing under `NDEBUG`.
- Treat the `test_server_streaming.cpp` split as structural test maintenance, not a behavior change to the daemon or broker runtime.
- Keep Tier 9 intentionally selective. Target `9.1`, `9.2`, `9.3`, `9.6`, and a bounded version of `9.7` directly. Leave `9.4`, `9.5`, `9.8`, `9.9`, and `9.10` opportunistic unless adjacent edits make them free.
- Keep commits medium-sized and reviewable. Do not create empty commits during execution.

**Architecture:** Use one bounded production cleanup pass for the selected Tier 9 items, one low-risk maintenance pass for the small Tier 8 dedup findings, and one dedicated C++ test-infrastructure pass for the broad `assert()` problem plus the `test_server_streaming` monolith split. Prefer a lightweight in-tree test assertion helper over importing a framework. Keep the existing `test_server_streaming` host-test entrypoint shape and build wiring where possible, but split the implementation across scenario-grouped translation units through the existing `mk/sources.mk` registration path. On the Rust side, keep Tier 9 local to comments, helper shape, and small configuration seams rather than reopening broader transport or port-forward design.

**Verification Strategy:** Run focused broker and C++ host-test commands after each task, then finish with the repo quality gates required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `make -C crates/remote-exec-daemon-cpp check-posix`.

**Assumptions / Open Questions:**
- `mk/sources.mk` is the main registration point for splitting `test_server_streaming.cpp`, but execution should confirm whether `mk/host-tests.mk` or other make entrypoints need companion updates.
- A header-only test assertion helper is sufficient for Tier 8.1; no external framework should be necessary.
- For Tier 9.7, a small internal request/log context helper inside `daemon_client.rs` should be enough; this plan does not reopen the larger transport-layer split deferred from earlier tiers.

---

### Task 1: Selected Tier 9 Production Polish Sweep

**Intent:** Clear the verified low-risk Tier 9 items that are worth direct action now, while explicitly leaving the low-value or scope-expanding items alone.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`

**Notes / constraints:**
- In scope: `9.1`, `9.2`, `9.3`, `9.6`, and a bounded `9.7`.
- Keep `9.3` as documentation or naming clarification unless execution proves the asymmetry is actually wrong. Do not change protocol behavior in this tier.
- For `9.7`, prefer a small internal request/log context helper to reduce repeated closure shape without moving transfer or RPC responsibilities across modules.
- Out of scope unless genuinely free while touching the same code: `9.4`, `9.5`, `9.8`, `9.9`, `9.10`.

**Verification:**
- Run: `cargo test -p remote-exec-broker`
- Expect: broker unit tests and integration behavior around daemon client and port forwarding remain green.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: the `connection` header token scan cleanup remains behavior-neutral for the C++ HTTP/tunnel path.

- [ ] Reconfirm the selected Tier 9 items are still live in the current files and keep the out-of-scope items explicitly untouched
- [ ] Apply the bounded production cleanups without changing public behavior or protocol semantics
- [ ] Add comments or tiny helper reshapes only where they make an intentional race or asymmetry authoritative
- [ ] Run focused verification
- [ ] Commit

### Task 2: Tier 8 Low-Risk Test Dedup And Registry Contract Cleanup

**Intent:** Resolve the small still-live Tier 8 items without getting pulled into the broader test-infrastructure rewrite yet.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/tests/[confirm narrow shared helper header if needed]`
- Likely modify: `crates/remote-exec-broker/src/tools/registry.rs`
- Existing references: `crates/remote-exec-broker/src/mcp_server.rs`

**Notes / constraints:**
- Covers Tier `8.3` and `8.4` only.
- Reuse the existing shared test seam for `make_config`; do not introduce a new generic test support layer unless the current sharing point is genuinely insufficient.
- Remove the third hand-maintained tool-name copy, but keep the contract test meaningful. Prefer deriving the expected set from the live registry/enum shape rather than replacing one duplicate with another.

**Verification:**
- Run: `cargo test -p remote-exec-broker`
- Expect: the registry/unit tests still prove the broker tool surface is internally consistent.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: extracting the duplicated config helper does not change streaming-test behavior.

- [ ] Confirm the exact helper duplication seam and the current tool-name contract-test shape
- [ ] Deduplicate the C++ `make_config` helper through the existing shared test surface
- [ ] Collapse the third broker tool-name copy while preserving a real contract check
- [ ] Run focused verification
- [ ] Commit

### Task 3: Introduce C++ Test Assertion Infrastructure And Migrate The Suite

**Intent:** Address Tier 8.1 as a dedicated test-infrastructure pass, not as incidental cleanup.

**Relevant files/components:**
- Likely create: `crates/remote-exec-daemon-cpp/tests/test_assert.h`
- Likely modify: most files under `crates/remote-exec-daemon-cpp/tests/`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/tests/test_socket_pair.h`

**Notes / constraints:**
- Covers Tier `8.1`.
- Keep the solution header-only or otherwise lightweight and fully in-tree.
- The new assertion helper must remain active under `NDEBUG`, preserve readable file/line failure context, and stay compatible with the current single-binary host-test style.
- Because the current suite has a broad `assert()` footprint, prefer a consistent migration pass rather than leaving a mixed long-term style across adjacent tests.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the host test suite still builds and runs cleanly, and assertion behavior no longer depends on debug-only semantics.

- [ ] Confirm the current assertion footprint and choose the narrowest helper shape that still fixes the `NDEBUG` problem
- [ ] Introduce the test assertion helper without pulling in an external framework
- [ ] Migrate the affected C++ tests to the new assertion surface in a coherent pass
- [ ] Run focused verification
- [ ] Commit

### Task 4: Split `test_server_streaming` By Scenario Group And Finish The Tier 8/9 Sweep

**Intent:** Break up the remaining Tier 8 monolith while preserving coverage and build entrypoint behavior, then close with the full quality gates.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Likely create: scenario-grouped `crates/remote-exec-daemon-cpp/tests/test_server_streaming_*.cpp` files
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/mk/host-tests.mk`
- Likely inspect: `crates/remote-exec-daemon-cpp/GNUmakefile`
- Likely inspect: `crates/remote-exec-daemon-cpp/NMakefile`

**Notes / constraints:**
- Covers Tier `8.2` and any residual Tier `8.1` fallout localized to the streaming tests.
- Preserve the current `test_server_streaming` host-test binary name unless execution proves the build system needs a different arrangement.
- Split by scenario boundary, not arbitrary line count. Likely groupings include TCP forwarding, UDP forwarding, retained-session lifecycle, timeouts/limits, and runtime glue.
- Keep shared helpers narrow. Do not replace one `.cpp` monolith with a giant support header that hides the same complexity.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: the split test set preserves the current effective streaming coverage.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: all C++ host tests and the production daemon build still pass after the file split.
- Run: `cargo test --workspace`
- Expect: cross-language workspace behavior remains green after the Tier 8/9 bundle.
- Run: `cargo fmt --all --check`
- Expect: Rust formatting stays clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no new Rust lint regressions are introduced.

- [ ] Confirm the current streaming-test scenario boundaries and the exact build-file touch points
- [ ] Split `test_server_streaming.cpp` into grouped translation units while keeping the host-test target shape stable
- [ ] Re-run the verified Tier 8/9 queries and confirm the final code shape matches the intended scope
- [ ] Run the full required quality gates
- [ ] Commit any real follow-up change; do not create an empty sweep commit
