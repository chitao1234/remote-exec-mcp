# Section 5 Test Layout Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Improve test navigability in the verified section-5 hotspots without widening production visibility or turning white-box unit coverage into brittle integration tests.

**Requirements:**
- Limit this pass to verified section-5 items from `docs/code-quality-audit-2026-05-16.md`.
- Treat test placement as a readability and boundary issue, not as a reason to move white-box unit tests into crate-level `tests/` by default.
- Include low-risk deduplication when a cited test file is already in the right structural place and the duplication is obvious.
- Preserve current behavior, current test coverage intent, and current public/internal API visibility unless a small test-support seam is clearly justified.
- Keep `remote-exec-host/src/port_forward/port_tunnel_tests.rs` in the host port-forward module tree for this pass; do not force it into crate-level integration tests.
- Treat `remote-exec-host/src/exec/store/tests.rs` as already structurally acceptable; do not churn it just to satisfy the audit wording.

**Architecture:** The cleanup should distinguish between three cases. First, very large inline unit-test blocks that dominate their production files should move into sibling `tests.rs` or `module/tests.rs` files so the production module becomes readable again while tests keep access to internal items. Second, files already using an out-of-line test module should stay in that shape, with only local dedup improvements where repetition is materially obscuring the intent. Third, no task in this pass should require broad `pub(crate)` expansion or migration to crate-level integration tests.

**Verification Strategy:** Run focused tests for each touched area as the tasks land, then finish with format and targeted lint checks if the final code motion creates warnings. Verify that test movement does not change feature gating, Windows-specific coverage, or broker/daemon config parsing behavior.

**Assumptions / Open Questions:**
- `tcp_bridge.rs`, `broker config.rs`, and `exec/shell/windows.rs` are the strongest structural split candidates because inline tests materially dominate the production file.
- `daemon/src/config/tests.rs` should stay where it is; the actionable work there is helper extraction and table-style dedup, not relocation.
- If a split would require widening production visibility beyond private module-sibling access, implementation should stop and narrow the task instead of forcing the move through.

---

### Task 1: Split broker-side inline test hotspots into sibling test modules

**Intent:** Remove the two largest verified inline test blocks from production-heavy broker files while preserving white-box unit-test access.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/tcp_bridge/tests.rs`
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely create: `crates/remote-exec-broker/src/config/tests.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Notes / constraints:**
- Prefer sibling `tests.rs` modules over crate-level `tests/` integration tests.
- Keep production symbol visibility as-is where module-sibling tests can already reach what they need.
- Treat this as mostly code motion plus any tiny imports/support reshaping needed to keep the tests local and readable.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-broker --lib config`
- Expect: broker port-forward behavior and config loading tests still pass, with no public API churn.

- [ ] Confirm the exact inline test block boundaries and any helper functions they currently rely on
- [ ] Move broker config tests into `config/tests.rs` with module-private access preserved
- [ ] Move broker TCP bridge tests into `tcp_bridge/tests.rs` with module-private access preserved
- [ ] Run focused broker verification
- [ ] Commit

### Task 2: Split host Windows shell inline tests and keep existing out-of-line host test structure stable

**Intent:** Reduce file bloat in `exec/shell/windows.rs` while explicitly rejecting unnecessary churn for the host test files that are already structurally acceptable.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/exec/shell/windows.rs`
- Likely create: `crates/remote-exec-host/src/exec/shell/windows/tests.rs`
- Existing references: `crates/remote-exec-host/src/exec/store.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/mod.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`

**Notes / constraints:**
- Do not move `exec/store/tests.rs` again; it is already using the preferred sibling-module shape.
- Do not migrate `port_tunnel_tests.rs` to crate-level `tests/` in this pass.
- Keep Windows-specific unit tests in the module tree so they can continue exercising private shell-resolution helpers without widening visibility.

**Verification:**
- Run: `cargo test -p remote-exec-host exec::shell::windows`
- Run: `cargo test -p remote-exec-host exec::store`
- Expect: Windows shell-selection coverage remains intact, and no unrelated host-test layout churn occurs.

- [ ] Confirm the existing Windows shell test helper usage and module-private access needs
- [ ] Move the inline Windows shell tests into `windows/tests.rs`
- [ ] Leave `exec/store/tests.rs` and `port_tunnel_tests.rs` structurally unchanged in this pass
- [ ] Run focused host verification
- [ ] Commit

### Task 3: Deduplicate repetitive daemon and broker config test setup without broad redesign

**Intent:** Shrink the repetitive config-test setup that section 5 correctly flags, while keeping the test logic in-place and avoiding a speculative config-test framework.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/src/config/tests.rs`
- Likely modify: `crates/remote-exec-broker/src/config/tests.rs`
- Existing references: `crates/remote-exec-daemon/src/config/mod.rs`
- Existing references: `crates/remote-exec-broker/src/config.rs`

**Notes / constraints:**
- Favor small helpers for TOML fixture assembly, write/load helpers, and repeated assertions over large macro layers.
- Use table-driven cases only where several tests differ by one or two fields and the parameterization remains readable.
- Do not mix in unrelated config-model redesign while touching these tests.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --lib config`
- Run: `cargo test -p remote-exec-broker --lib config`
- Expect: the config suites remain behaviorally identical, but fixture setup becomes shorter and easier to scan.

- [ ] Identify the highest-value duplicated fixture/setup patterns in daemon and broker config tests
- [ ] Extract low-risk shared helpers and table-style cases where they clearly improve readability
- [ ] Keep assertions explicit where table-driven encoding would make failures harder to read
- [ ] Run focused daemon and broker config verification
- [ ] Commit

### Task 4: Final section-5 sweep and formatting check

**Intent:** Confirm the section-5 cleanup stayed within the approved boundary and did not accidentally widen internal visibility or degrade test organization elsewhere.

**Relevant files/components:**
- Likely review: `crates/remote-exec-broker/src/config.rs`
- Likely review: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely review: `crates/remote-exec-host/src/exec/shell/windows.rs`
- Likely review: `crates/remote-exec-daemon/src/config/tests.rs`

**Notes / constraints:**
- This sweep is specifically for scope control: accept local helper extraction, reject unrelated production refactors.
- Confirm that the new sibling test modules are wired through `#[cfg(test)] mod tests;` and remain feature-safe.

**Verification:**
- Run: `cargo fmt --all --check`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-daemon --lib config`
- Expect: final test layout compiles cleanly, formatting is stable, and the moved/deduped suites still pass.

- [ ] Review the final diff for visibility widening or accidental production churn
- [ ] Run the final section-5 verification sweep
- [ ] Commit
