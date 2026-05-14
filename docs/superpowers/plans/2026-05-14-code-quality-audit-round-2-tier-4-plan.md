# Code Quality Audit Round 2 Tier 4 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Tier 4 lock-contention and hot-path performance issues from `docs/code-quality-audit-round-2.md` without changing the public broker surface, the v4 tunnel wire format, or daemon capability reporting.

**Requirements:**
- Verify every Tier 4 audit claim against the current tree and execute only the claims that are still live.
- Preserve public MCP tool arguments, result shapes, broker-owned `forward_id` namespaces, reconnect semantics, and the v4 port-forward tunnel protocol.
- Keep behavior aligned between broker, host runtime, and daemon where the live contract depends on current lifecycle ordering or timeout semantics.
- Prefer owner-local fixes that remove avoidable lock hold time or queue coupling; do not introduce a broad new abstraction layer to hide small hot-path fixes.
- Keep the established workflow: medium-sized tasks, focused verification after each task, no worktrees, no empty commits.
- Treat `docs/code-quality-audit-round-2.md` as historical input only; do not modify it as part of this Tier 4 pass.

**Architecture:** Treat Tier 4 as three implementation batches plus a confirmatory sweep. First, reshape broker port-forward store bookkeeping so reconnect-capacity checks are O(1) and close-path bookkeeping is separated from network I/O. Second, tighten broker tunnel shutdown and heartbeat internals so reader-side control traffic does not block on a full writer queue and tunnel-stop waits consume one total timeout budget instead of three serialized budgets. Third, narrow daemon startup validation so the existing validated-wrapper surface remains intact but host-runtime validation and conversion no longer require cloning the whole daemon config just to validate borrowed fields.

**Verification Strategy:** Run focused broker and daemon tests after each batch, then finish with the quality gates required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `make -C crates/remote-exec-daemon-cpp check-posix`.

**Assumptions / Open Questions:**
- Audit item `4.2` should preserve the current close API contract, including validation of requested IDs and stop-on-first-close-failure semantics, unless focused verification proves that a broader behavior change is already intended.
- Audit item `4.5` should be narrowed to the reader-side heartbeat echo only. Data-plane frame backpressure should remain enforced exactly as today.
- Audit item `4.4` is narrower than the original audit wording after the validated-wrapper work: the real remaining issue is clone-heavy validation/conversion, not absence of a validated wrapper.
- If the simplest `4.5` fix is a best-effort `try_send` for heartbeat acks, confirm during execution that dropping a single ack under sustained write pressure does not regress the intended reconnect behavior more than the current reader stall does.

**Planning-Time Verification Summary:**
- `4.1`: valid and in scope. `crates/remote-exec-broker/src/port_forward/store.rs` still scans `entries.values()` under the write lock in `ensure_reconnect_capacity(...)`.
- `4.2`: valid and in scope. `PortForwardStore::close(...)` still holds `close_lock` while awaiting `close_handle(...)`, which performs tunnel shutdown and listener-close I/O.
- `4.3`: valid and in scope. `PortTunnel::wait_closed(...)` still acquires task handles and applies the full timeout to reader, writer, and heartbeat tasks sequentially.
- `4.4`: partially valid and narrowed. `DaemonConfig::validate()` still calls `HostRuntimeConfig::from(self.clone()).validate()?`, and daemon startup still converts from an owned config after validation, so startup still pays full-config clone cost in the validation path.
- `4.5`: valid and in scope. The broker tunnel reader still handles inbound `TunnelHeartbeat` frames by awaiting `reader_tx.send(...)` on the same bounded writer queue used for ordinary outbound frames.

---

### Task 1: Remove Broker Store Hot-Lock Paths In Reconnect And Close Flows

**Intent:** Eliminate the store-level Tier 4 contention points by making reconnect-capacity checks O(1) and by separating close-batch map bookkeeping from the async close work.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Likely inspect: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely inspect: broker port-forward store tests embedded in `crates/remote-exec-broker/src/port_forward/store.rs`
- Existing integration coverage: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Notes / constraints:**
- Cover audit items `4.1` and `4.2`.
- Preserve the public meaning of `ForwardPortPhase::{Ready, Reconnecting, Failed, Closed}` and the existing reconnect-limit error string.
- Keep close-ID validation behavior and the current “mark failed and return error” behavior on close failure unless focused execution verification shows the live contract expects broader batch progress.
- If a reconnecting counter is introduced, it must stay coherent across insert, mark-ready, mark-reconnecting, mark-failed, close/remove, and drain paths.
- Avoid replacing one broad lock with a more implicit synchronization hazard. The fix should make ownership clearer, not just shift contention around.

**Verification:**
- Run: `cargo test -p remote-exec-broker --lib port_forward::store::tests::mark_reconnecting_fails_new_forward_when_reconnect_limit_is_reached -- --exact --nocapture`
- Expect: reconnect-limit enforcement still fails the new forward exactly when the configured capacity is already consumed.
- Run: `cargo test -p remote-exec-broker --lib [new or updated store close-path test] -- --exact --nocapture`
- Expect: unrelated close work is no longer serialized behind network I/O while ID validation and failure reporting remain correct.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker-facing forward open/list/close/reconnect behavior remains unchanged.

- [ ] Reconfirm the current reconnect-capacity and close-batch bookkeeping paths and identify the exact phase transitions that affect reconnecting capacity
- [ ] Refactor store state so reconnect-capacity checks stop scanning the full entry map under the write lock
- [ ] Reshape the close path so entry selection/removal happens under lock but close-handle I/O happens after that lock is released
- [ ] Add or update unit coverage for reconnect accounting and close-path lock release behavior
- [ ] Run the focused broker verification
- [ ] Commit with real changes only

### Task 2: Tighten Broker Tunnel Shutdown And Heartbeat Backpressure Handling

**Intent:** Bound tunnel shutdown to one timeout budget and decouple reader-side heartbeat echo handling from ordinary writer-queue backpressure so control traffic does not stall wire reads.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely inspect: `crates/remote-exec-broker/src/port_forward/timings.rs`
- Likely inspect: broker tunnel unit tests embedded in `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Existing integration coverage: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Notes / constraints:**
- Cover audit items `4.3` and `4.5`.
- Preserve current retryable transport classification and the externally visible reconnect behavior after true heartbeat timeout.
- Only relax queue coupling for internally generated heartbeat-ack control frames. Ordinary outbound frames should still obey the existing queue-byte budget and bounded writer channel semantics.
- `wait_closed(...)` should stop multiplying the configured timeout by the number of background tasks; the total wait should be bounded by one caller-supplied timeout budget.
- If task-handle extraction changes, keep repeated `wait_closed(...)` or `abort()+wait_closed(...)` behavior well defined and idempotent.

**Verification:**
- Run: `cargo test -p remote-exec-broker --lib port_forward::tunnel::tests::heartbeat_timeout_surfaces_retryable_transport_error -- --exact --nocapture`
- Expect: true heartbeat timeout still surfaces as the existing retryable transport failure.
- Run: `cargo test -p remote-exec-broker --lib [new or updated wait_closed timeout test] -- --exact --nocapture`
- Expect: tunnel shutdown consumes one total timeout budget rather than three serialized budgets.
- Run: `cargo test -p remote-exec-broker --lib [new or updated heartbeat-echo backpressure test] -- --exact --nocapture`
- Expect: a full outbound queue no longer blocks the reader task from continuing to drain inbound frames.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker reconnect and close behavior remains stable under the changed tunnel internals.

- [ ] Reconfirm the current heartbeat reader path and sequential `wait_closed(...)` timeout behavior in the broker tunnel
- [ ] Refactor tunnel shutdown waiting so one timeout budget covers reader, writer, and heartbeat task termination together
- [ ] Change reader-side heartbeat echo handling so it no longer awaits bounded writer-queue capacity on the hot read path
- [ ] Add or update unit coverage for shutdown timing and heartbeat-echo backpressure behavior
- [ ] Run the focused broker verification
- [ ] Commit with real changes only

### Task 3: Remove Clone-Heavy Daemon Validation And Host Conversion

**Intent:** Preserve the validated daemon-config boundary while removing the remaining whole-config clone from daemon validation and startup-state preparation.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Likely modify: `crates/remote-exec-daemon/src/lib.rs`
- Likely inspect: `crates/remote-exec-host/src/config/mod.rs`
- Existing coverage: `crates/remote-exec-daemon/tests/health.rs`
- Existing compile coverage: workspace tests and clippy

**Notes / constraints:**
- Cover the narrowed live portion of audit item `4.4`.
- Preserve the current `ValidatedDaemonConfig` public shape and the existing `DaemonConfig::load(...)` / `into_validated(...)` behavior.
- Keep path normalization, filesystem directory validation, transfer-limit validation, yield-time validation, HTTP auth validation, and TLS validation behavior aligned with current startup semantics.
- Prefer a borrowed host-validation helper or borrowed conversion view over introducing another parallel config struct that would just duplicate fields again.
- Do not expand this task into a broader daemon/host config redesign. The goal here is to remove the clone-heavy validation seam, not to reopen the daemon-boundary architecture work.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: daemon startup validation, plain HTTP/TLS validation, and target-info behavior remain unchanged.
- Run: `cargo test -p remote-exec-host [relevant config tests if touched]`
- Expect: host config validation still rejects invalid workdirs, session limits, and port-forward limits correctly.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: the borrowed validation/conversion path does not introduce new lint regressions.

- [ ] Reconfirm the current daemon validation and startup-state construction path, including where owned conversion still forces full-config cloning
- [ ] Introduce the narrow borrowed validation/conversion helper(s) needed to avoid cloning the full daemon config just to validate host-runtime fields
- [ ] Keep daemon startup and validated-wrapper behavior unchanged while simplifying the clone-heavy path
- [ ] Add or update focused daemon/host validation coverage if the new helper changes the seam materially
- [ ] Run the focused daemon verification
- [ ] Commit with real changes only

### Task 4: Final Tier 4 Sweep And Full Quality Gate

**Intent:** Reconfirm which Tier 4 seams were fixed or intentionally narrowed, then finish with the full Rust and relevant C++ quality gates before declaring Tier 4 complete.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit-round-2.md`
- Likely inspect: touched broker store/tunnel and daemon config files from Tasks 1 through 3

**Notes / constraints:**
- Keep the sweep limited to Tier 4 of `docs/code-quality-audit-round-2.md`; do not expand into Tier 1, Tier 2, or unrelated Round 5 work during this pass.
- Reconfirm that any remaining lock, queue, or clone behavior exists because the live boundary requires it, not because a verified Tier 4 weak seam was left behind accidentally.
- If one item stays intentionally narrowed rather than fully refactored, record that explicitly in the implementation summary instead of forcing a risky late redesign.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the full Rust workspace passes end-to-end after the Tier 4 bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: touched Rust changes do not break the shared POSIX C++ quality gate.

- [ ] Re-run the Tier 4 verification queries and confirm the final code shape is intentional for each audited seam
- [ ] Run the required Rust workspace quality gate
- [ ] Run the relevant C++ POSIX quality gate
- [ ] Summarize which Tier 4 items were fixed, narrowed, or intentionally left unchanged
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
