# Code Quality Audit Round 2 Tier 2 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Tier 2 correctness issues from `docs/code-quality-audit-round-2.md` with medium-sized batches that improve internal state ownership and lifecycle behavior without changing the public broker surface or the v4 port-forward wire format.

**Requirements:**
- Verify every Tier 2 audit claim against the current tree and execute only the claims that are still live.
- Preserve public MCP tool arguments, result shapes, warning/error wire strings, broker-owned public ID namespaces, and the v4 port-forward tunnel protocol.
- Keep broker, Rust host/daemon, and C++ daemon behavior aligned where they share the same broker-daemon contract.
- Prefer owner-local refactors that remove correctness hazards at the responsible seam; do not introduce a broad new abstraction layer to hide these fixes.
- Continue the established workflow: medium-sized tasks, focused verification after each task, no worktrees, no empty commits.
- Treat `docs/code-quality-audit-round-2.md` as historical input only; do not modify it as part of this remediation pass.

**Architecture:** Treat Tier 2 as three implementation batches plus a confirmatory sweep. First, fix the small but real correctness seams in wire-code mapping and exec-session warning/insert behavior, including the parallel Rust and C++ session-limit logic. Second, make tunnel/listen control state authoritative and atomic at the owner level so role, protocol, generation, and active runtime cannot be composed from torn reads during reconnect and close flows. Third, harden shutdown and resource cleanup paths so partially opened tunnels, background task drains, C++ counter underflow, and broker TCP active-stream release all have explicit lifecycle handling instead of opportunistic cleanup.

**Verification Strategy:** Run focused verification after each task, then finish with the quality gates required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `make -C crates/remote-exec-daemon-cpp check-posix`.

**Assumptions / Open Questions:**
- Audit item `2.4` is still live, but it is primarily a state-model defect rather than a known user-visible failure today; the fix should remove the split authoritative state rather than only papering over the current error path.
- Audit item `2.5` should be fixed in the same batch as `2.4` because both issues come from torn reads across separately owned state fields in port-forward lifecycle control.
- Audit item `2.7` can likely be fixed either by explicit connect-tunnel abort on listen-open failure or by deferring connect-tunnel open until after listen readiness; confirm the smaller-risk path during execution.
- For `2.9`, choose explicit fatal behavior deliberately. Silent underflow in release builds is not acceptable, but crashing the whole daemon should be justified if that remains the selected policy.

**Planning-Time Verification Summary:**
- `2.1`: valid and in scope. `crates/remote-exec-proto/src/rpc/error.rs` still keeps the RPC error-code wire mapping in both `RPC_ERROR_CODE_WIRE_VALUES` and `RpcErrorCode::wire_value()`.
- `2.2`: valid and in scope. Rust still derives the warning threshold from `DEFAULT_SESSION_LIMIT`, and C++ still derives it from `DEFAULT_MAX_OPEN_SESSIONS`, so configured non-default limits produce wrong or dead warnings.
- `2.3`: valid and in scope. `store_running_session(...)` still inserts into the host session store and then re-locks by ID instead of receiving a lease directly from the insert operation.
- `2.4`: valid and in scope. Host port-forward `active_access(...)` still reads tunnel mode and active runtime/session context through separate locks and separate helper calls.
- `2.5`: valid and in scope. Broker listen-session generation and current tunnel are still stored separately and updated non-atomically.
- `2.6`: valid and in scope. Broker `close_listen_session(...)` still treats a missing retained tunnel as a signal to reconnect and close remotely.
- `2.7`: valid and in scope. Broker forward open still retains a connect tunnel before listener readiness is confirmed and does not explicitly abort that connect tunnel on listen-open failure.
- `2.8`: valid and in scope. Host `BackgroundTasks::join_all()` still holds the `JoinSet` mutex while awaiting all joins.
- `2.9`: valid and in scope. C++ `release_counter(...)` still logs and then relies on `assert(false)` for an exhausted counter path.
- `2.10`: valid and in scope. Broker TCP bridge still manually calls `release_active_tcp_stream(...)` from multiple branches instead of expressing release as a guaranteed scope/lifecycle action.

---

### Task 1: Remove Small Correctness Duplication And Exec-Session Store Hazards

**Intent:** Eliminate the still-live low-risk Tier 2 correctness issues in RPC error-code mapping and exec-session warning/insert behavior, covering both Rust and C++ where the contract is shared.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/rpc/error.rs`
- Likely modify: `crates/remote-exec-host/src/exec/store.rs`
- Likely modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely inspect: `crates/remote-exec-daemon-cpp/include/session_store.h`
- Likely inspect: `crates/remote-exec-host/src/exec/session.rs`
- Likely inspect: existing proto and exec-session tests in `remote-exec-proto`, `remote-exec-host`, `remote-exec-daemon`, and `remote-exec-daemon-cpp`

**Notes / constraints:**
- Cover audit items `2.1`, `2.2`, and `2.3`.
- Preserve all existing serialized RPC error code strings, including alias handling such as `"internal"` versus `"internal_error"`.
- Compute warning thresholds from the configured session limit, not from a default constant, and keep warning text aligned with the actual threshold used by enforcement.
- Prefer making `SessionStore::insert(...)` return the inserted lease and warning-threshold outcome together rather than layering another lookup helper around the existing API.
- Keep any host-session API change local to the owner crate unless a wider surface is actually required by tests.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Expect: RPC error-code round-trip and alias behavior stays stable.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: daemon exec-session behavior and warning output still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker-facing exec behavior remains unchanged.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Expect: C++ session-store warnings and retention behavior still pass.

- [ ] Reconfirm the exact `2.1`, `2.2`, and `2.3` seams in the current code and identify the focused regression tests that cover them
- [ ] Collapse RPC error-code encode/decode mapping to one authoritative source while preserving existing wire strings and aliases
- [ ] Derive exec warning thresholds from the configured limit in both Rust and C++, and keep warning messages consistent with that threshold
- [ ] Change the Rust host session-store insert path to hand back the inserted lease so `store_running_session(...)` stops doing a second lookup
- [ ] Run the focused proto, daemon exec, broker exec, and C++ session-store verification
- [ ] Commit with real changes only

### Task 2: Make Port-Forward Tunnel And Listen-Session State Atomic At The Owner Boundary

**Intent:** Remove the torn-read state model in host and broker port-forward lifecycle control so mode, protocol, active context, generation, and retained tunnel are read and updated as authoritative owner-local state.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/types.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/active.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely inspect: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely inspect: `crates/remote-exec-host/src/port_forward/udp.rs`

**Notes / constraints:**
- Cover audit items `2.4`, `2.5`, and `2.6`.
- On the host side, prefer one authoritative tunnel-active state object that expresses unopened versus connect versus listen, including protocol plus runtime/session handle, instead of separate `open_mode` and `active` locks.
- On the broker side, move listen-session generation into the same mutex-owned state as the retained tunnel, and update read helpers so callers do not compose `current_generation()` and `current_tunnel()` independently when they need one coherent snapshot.
- Keep reconnect semantics and resume-session behavior unchanged except for the verified correctness fix: missing retained tunnel during close should not trigger a fresh reconnect solely to send a close.
- Preserve current broker-visible forward phases, session IDs, and generation semantics.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: host/daemon tunnel-open, close, and resume flows still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding, reconnect, and close behavior still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker behavior remains aligned when talking to the C++ daemon.

- [ ] Reconfirm the current host tunnel-state and broker listen-session ownership model, including the exact readers that currently compose torn state
- [ ] Refactor host port-forward tunnel state so protocol/role and active runtime are read from one authoritative owner-local state path
- [ ] Refactor broker listen-session state so generation and current tunnel are updated and read together when a coherent snapshot is required
- [ ] Make missing retained listen tunnel a no-op close path instead of a reconnect-to-close path
- [ ] Run the focused daemon and broker port-forward verification for Rust and C++ daemon coverage
- [ ] Commit with real changes only

### Task 3: Harden Partial-Open, Shutdown, And Active-Resource Cleanup Paths

**Intent:** Make the remaining lifecycle-sensitive Tier 2 paths explicit and robust so partial-open tunnels, background task shutdown, counter underflow, and active TCP stream accounting no longer depend on fragile branch ordering.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-host/src/state.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Likely inspect: `crates/remote-exec-daemon-cpp/include/port_tunnel.h`
- Likely inspect: broker and C++ tests covering tunnel lifecycle and port-tunnel resource accounting

**Notes / constraints:**
- Cover audit items `2.7`, `2.8`, `2.9`, and `2.10`.
- For the connect-tunnel leak, prefer the smallest safe fix that makes cleanup explicit; if restructuring open order is materially riskier than abort-on-failure, use the explicit abort path.
- `BackgroundTasks::join_all()` should stop awaiting joins while holding the `JoinSet` mutex, but it should still tolerate background tasks that fail during shutdown and keep current warning behavior.
- Replace the C++ exhausted-counter behavior with an explicit, release-build-effective policy. If it remains fatal, the code should say so directly rather than depending on debug-only `assert`.
- Express broker active TCP stream release as an unconditional lifecycle action, such as a guard or single-exit cleanup pattern, rather than duplicating release calls across error branches.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker tunnel-open failure, reconnect, and TCP stream cleanup behavior still passes.
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: host shutdown and background task drain behavior remains healthy.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: C++ port-tunnel upgrade, limits, and lifecycle behavior still passes.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: runtime integration remains correct after any port-tunnel lifecycle change.

- [ ] Reconfirm the current partial-open, shutdown, and active-stream cleanup paths and the tests that exercise them
- [ ] Make failed listen-open explicitly clean up any already-opened connect tunnel
- [ ] Refactor host background-task shutdown so joins happen outside the `JoinSet` mutex while preserving failure logging
- [ ] Replace C++ counter-underflow handling with explicit release-build-effective behavior
- [ ] Refactor broker TCP active-stream release into a guaranteed cleanup shape instead of repeated branch-local calls
- [ ] Run the focused broker, daemon health, and C++ port-tunnel/runtime verification
- [ ] Commit with real changes only

### Task 4: Final Tier 2 Sweep And Full Quality Gate

**Intent:** Confirm that all verified Tier 2 claims were fixed or intentionally narrowed in code, then run the full Rust and touched C++ quality gates before declaring the Tier 2 remediation pass complete.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit-round-2.md`
- Likely inspect: the code paths touched by Tasks 1 through 3

**Notes / constraints:**
- Keep the sweep limited to Tier 2 of the new audit round; do not expand into Tier 1 or later tiers during this pass.
- Reconfirm that any remaining strings or split state are present only because the live contract or runtime boundary requires them, not because the previous weak internal seam was left in place.
- If one of the verified items needs to remain partially narrowed rather than fully refactored, record that explicitly in the implementation summary instead of forcing a risky late redesign.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the full Rust workspace passes.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: touched C++ code and host tests still pass.

- [ ] Re-run the Tier 2 verification queries and confirm the final code shape is intentional for each audited seam
- [ ] Run the required Rust workspace quality gate
- [ ] Run the relevant C++ POSIX quality gate
- [ ] Summarize which Tier 2 items were fixed, narrowed, or intentionally left unchanged
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
