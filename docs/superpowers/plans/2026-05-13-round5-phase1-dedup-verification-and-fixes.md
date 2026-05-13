# Round 5 Phase 1 Dedup Verification And Fixes Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the live Round 5 Phase 1 dedup seams that still exist in current Rust and C++ code, while correcting stale audit assumptions before implementation starts.

**Requirements:**
- Cover the verified current-code follow-up for audit items `#1` through `#7`.
- Preserve broker public behavior, daemon RPC semantics, transfer streaming behavior, and port-forward recovery behavior.
- Keep the work low-risk and local to the owning crates; avoid broad new abstraction layers unless the existing seam already supports them cleanly.
- Continue the user's plan-based execution style and commit after each real task with no empty commits.
- Do not rewrite `docs/CODE_AUDIT_ROUND5.md`; it is input to this plan, not a live contract.

**Architecture:** Treat this as three implementation batches plus a final sweep. First, clean up the stale daemon yield-time duplicate and collapse the duplicated daemon-client success guards while preserving strict vs lenient RPC body decoding. Second, deduplicate the Rust port-forward error/recovery helpers at their current owner boundaries instead of forcing a new cross-cutting trait hierarchy. Third, collapse the broker transfer single-source branching and the C++ session retirement sequence without changing observable transfer or session-lifecycle behavior.

**Verification Strategy:** Run focused verification after each task using the direct unit/integration suites for the touched seams, then finish with the cross-cutting quality gate required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and the relevant C++ POSIX checks.

**Assumptions / Open Questions:**
- Audit item `#1` is only partially current: `crates/remote-exec-daemon/src/config/mod.rs` already re-exports `remote_exec_host` yield-time types, so the live fix is removing the orphaned duplicate file after confirming it has no hidden references.
- Audit item `#3` is directionally correct, but one TCP connect-side branch performs extra stream cleanup before reconnecting; any helper must preserve that hook.
- Audit item `#4` is directionally correct, but the generic RPC path uses `decode_rpc_error_strict()` while transfer paths use lenient decoding; the dedup must keep that policy split explicit.
- For audit item `#2`, prefer a narrow shared error-frame builder or sender adapter over introducing a larger trait unless execution shows the broader abstraction is already the smallest clean change.

**Planning-Time Verification Summary:**
- `#1`: partially stale as written; active double-maintenance is already gone, but the dead duplicate file remains.
- `#2`: verified live in `remote-exec-host` port-forward error helpers.
- `#3`: verified live with a branch-specific cleanup caveat.
- `#4`: verified live with a strict-vs-lenient decode caveat.
- `#5`: verified live in the TCP/UDP outer recovery loops.
- `#6`: verified live in `transfer_single_source`.
- `#7`: verified live in C++ `SessionStore` teardown paths.

---

### Task 1: Clean Up Yield-Time Remnant And Deduplicate Daemon Client Success Guards

**Intent:** Remove the stale duplicate daemon yield-time file and collapse the repeated HTTP success/error-status handling in the broker daemon client without changing current decode semantics.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Likely modify: `crates/remote-exec-daemon/src/config/tests.rs`
- Likely delete: `crates/remote-exec-daemon/src/config/yield_time.rs`
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`

**Notes / constraints:**
- Confirm the daemon crate still has no `mod yield_time;` declaration before deleting the orphaned file.
- Keep `YieldTimeConfig`, `YieldTimeOperation`, and `YieldTimeOperationConfig` sourced from `remote_exec_host`.
- Preserve the current difference between transfer status handling and generic RPC status handling by keeping strict-vs-lenient body decode policy explicit in the shared helper.
- Keep existing warning fields and message wording stable unless a helper naturally centralizes them without behavior drift.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --lib config::tests`
- Expect: daemon config parsing and yield-time defaults still pass with the host re-export as the only source.
- Run: `cargo test -p remote-exec-broker --lib daemon_client::tests`
- Expect: broker daemon-client unit tests still pass, including timeout/error decoding coverage.

- [ ] Reconfirm that the daemon yield-time module is an orphaned duplicate rather than an active module
- [ ] Remove the dead daemon copy and keep the daemon config surface on the host re-export path
- [ ] Replace the three near-identical daemon-client success guards with one shared helper that preserves decode-policy differences
- [ ] Add or extend daemon-client tests if the shared helper needs direct coverage for strict vs lenient error decoding
- [ ] Run focused verification
- [ ] Commit with real changes only

### Task 2: Deduplicate Rust Port-Forward Error And Recovery Control Flow

**Intent:** Reduce the repeated Rust port-forward error-frame and receive/recovery control-flow code while preserving reconnect, generation, and stream-cleanup behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/types.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/events.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`

**Notes / constraints:**
- Preserve `generation: Some(...)` on tunnel-owned error frames and `generation: None` on sender-only error frames.
- Do not change `PortTunnelClosed`, `InvalidPortTunnel`, or retryable transport classification behavior.
- Any shared receive/recovery helper must support the TCP connect-side `close_active_tcp_listen_streams(...)` hook before reconnect and the UDP dropped-datagram accounting before connect-side recovery.
- Keep the helper at the current broker port-forward owner boundary; do not broaden it into a new framework or public abstraction.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon/host port-forward RPC flows still pass after host-side helper extraction.
- Run: `cargo test -p remote-exec-broker --lib port_forward::`
- Expect: broker port-forward unit coverage still passes across TCP and UDP bridge behavior.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker public forwarding behavior still passes against the Rust daemon path.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker public forwarding behavior still passes against the C++ daemon path.

- [ ] Extract the narrowest shared host helper for building/sending tunnel error frames without changing sender ownership semantics
- [ ] Collapse the repeated broker receive/recover match blocks into one helper with explicit retry hooks where needed
- [ ] Collapse the duplicated TCP/UDP outer recovery loop structure into one shared runner or local helper while preserving protocol-specific side effects
- [ ] Run focused Rust forwarding verification across unit and public integration coverage
- [ ] Commit with real changes only

### Task 3: Collapse Single-Source Transfer Branching And C++ Session Retirement Duplication

**Intent:** Remove the repeated single-source transfer dispatch logic in the broker and the repeated session retirement sequence in the C++ daemon while preserving current streaming and lifecycle behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`

**Notes / constraints:**
- Preserve the current no-temp-file streaming fast paths for single-source transfers; do not regress local/remote single-source copies to archive-on-disk behavior just to simplify control flow.
- Keep target verification, sandbox checks, compression handling, and returned `TransferSourceType` behavior unchanged.
- For the C++ cleanup, keep map erase/remove ownership at the call sites and extract only the common "retire and join" sequence.
- Reuse existing broker transfer helpers where they already express archive import/export ownership cleanly instead of inventing a parallel abstraction.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: single-source transfer behavior still passes across local/local, local/remote, remote/local, and remote/remote paths.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Expect: C++ session lifecycle and pruning behavior still passes with the shared retirement helper.

- [ ] Reconfirm which parts of `transfer_single_source` are structurally duplicated versus intentionally distinct for streaming
- [ ] Extract the smallest shared export/import flow that removes the repeated request-building and dispatch pattern without reintroducing temp-file staging
- [ ] Extract and apply one C++ session retirement helper across destructor, prune, and completed-session paths
- [ ] Run focused broker transfer and C++ session-store verification
- [ ] Commit with real changes only

### Task 4: Final Phase 1 Confirmatory Sweep

**Intent:** Verify that the implemented Round 5 Phase 1 dedup bundle actually removed the targeted seams and still satisfies the repo quality gates for cross-cutting Rust and C++ changes.

**Relevant files/components:**
- Likely inspect: `docs/CODE_AUDIT_ROUND5.md`
- Likely inspect: the code paths touched by Tasks 1 through 3

**Notes / constraints:**
- Use grep or diff checks to confirm the targeted duplicate seams are actually gone or intentionally reduced to one owner.
- Keep this as a verification/sweep task; do not opportunistically widen scope into later Round 5 phases.
- If a supposedly removed seam still survives because the safe helper boundary was narrower than the audit proposed, document that in the implementation notes rather than forcing a risky abstraction.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the Rust workspace passes end-to-end after the full Phase 1 bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting stays clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no clippy regressions.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: C++ POSIX builds and host tests pass after the session-store cleanup.

- [ ] Re-run targeted searches for the audited duplicate seams and confirm the surviving code shape is intentional
- [ ] Run the full Rust quality gate required for cross-cutting changes
- [ ] Run the relevant C++ POSIX quality gate
- [ ] Summarize any audit items that proved partially stale versus fully remediated
- [ ] Commit any final sweep-only adjustments if needed, otherwise do not create an empty commit
