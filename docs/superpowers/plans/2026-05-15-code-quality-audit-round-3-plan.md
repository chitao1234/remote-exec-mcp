# Code Quality Audit Round 3 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Close the still-actionable round-3 audit items with three medium-sized implementation batches that prioritize POSIX correctness first, then broker forward robustness, then the remaining smaller cleanup seams.

**Requirements:**
- Exclude previously deferred structural refactors: `3.7` (`broker/src/daemon_client.rs`) and `3.8` (`daemon-cpp/include/live_session.h` and related ownership redesign).
- Keep the public MCP surface, broker-daemon RPC contract, and port-forward wire format unchanged.
- Preserve current behavior unless the audit item is specifically about correctness or safety. Do not turn this plan into a broad style cleanup pass.
- Treat the Tier 4 helper-noise items as out of scope for now unless a helper naturally disappears while implementing an in-scope fix.
- Keep Rust and C++ changes grouped into reviewable batches. Do not create an empty sweep commit.

**Architecture:** The verified round-3 work splits cleanly into three slices. First, the C++ daemon still has a compact set of POSIX correctness gaps around descriptor inheritance, signal inheritance, process-group setup, PTY initialization, and patch-file mode preservation; these are behaviorally important and low-risk to address together. Second, the broker still has one missing regression test and two port-forward internals whose current shape makes future mistakes easier, so that batch should harden the Rust-side forwarder without changing its public contract. Third, a smaller cleanup batch can handle the remaining worthwhile encapsulation and race fixes that are real but not urgent enough to block the first two passes.

**Verification Strategy:** Run focused C++ checks after the POSIX hardening batch and focused broker forwarding tests after the Rust forwarding batch. The final cleanup batch should run the narrow test suites for the touched Rust and C++ areas, then finish with the relevant cross-language quality gates for the files actually changed.

**Assumptions / Open Questions:**
- The C++ daemon already has adequate test seams for the POSIX fixes; implementation should add focused coverage only where current tests do not exercise the corrected path.
- The `queue_or_send_tcp_connect_frame` and UDP epoch-loop reshaping should stay internal-only. If either change starts to alter retry, logging, or backpressure semantics, stop and narrow the refactor.
- `3.10` (`unix_timestamp_string` pre-epoch fallback) and `3.14` (`WARNING_THRESHOLD_HEADROOM` policy) remain low-value unless execution uncovers a real contract or correctness reason to change them.

**Planning-Time Verification Summary:**
- In scope and verified: `3.1` sockets missing `CLOEXEC`, `3.2` `SIGPIPE` leaking into spawned children, `3.3` patch path dropping executable mode, `3.4` unchecked child-side `setpgid()`, `3.5` ignored initial `TIOCSWINSZ` result, `3.6` `/dev/null` missing `O_CLOEXEC`, `3.11` threshold check outside the lock, `3.12` overly wide `clear_on_transport_error`, `3.13` missing connect-tunnel abort regression test, plus the two still-worthwhile Tier 3 forwarder cleanups in `tcp_bridge.rs` and `udp_bridge.rs`.
- Explicitly deferred again: `3.7` `daemon_client.rs` transport/transfer split and `3.8` C++ `LiveSession` encapsulation are still real but belong to broader refactor passes, not this correctness-first plan.
- Explicitly not prioritized in this pass: `forward_exec_write` restart-detection extraction, `exec_start_response` reconstruction cleanup, `TcpReadLoopContext` accessor duplication, `unix_timestamp_string`, and warning-headroom policy churn.

---

### Task 1: Harden The C++ POSIX Runtime Paths

**Intent:** Fix the verified C++ daemon correctness and safety issues around inherited descriptors, inherited signals, PTY setup, patch-file mode retention, and child process-group setup.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/patch_engine.cpp`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/tests/`
- Existing reference: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`

**Notes / constraints:**
- Keep this batch POSIX-only. Do not mix in the broader `LiveSession` redesign or unrelated C++ cleanup.
- Preserve existing daemon behavior and route contracts. This batch is about safer runtime behavior, not feature changes.
- For the patch-mode fix, follow the existing transfer import pattern that preserves file mode rather than inventing a second policy.
- For `SIGPIPE`, restore the previous handler in the child before `exec` so the daemon stays hardened while spawned commands get normal POSIX behavior.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: POSIX daemon and test coverage remain green after the descriptor, signal, PTY, and patch changes.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: host/server streaming behavior remains green after the process-session and socket changes.

- [ ] Reconfirm each POSIX audit seam in the current tree and identify the narrowest shared helpers to touch
- [ ] Implement the `CLOEXEC`, `SIGPIPE`, `setpgid`, `TIOCSWINSZ`, `/dev/null`, and patch-mode fixes without changing public behavior
- [ ] Add or tighten tests only where the corrected path is not already exercised
- [ ] Run focused C++ verification
- [ ] Commit

### Task 2: Tighten Broker Forwarder Correctness And Regression Coverage

**Intent:** Lock in the connect-tunnel abort fix with a regression test and reshape the two still-risky broker forwarder internals so future edits do not silently leak state.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely inspect or modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Likely inspect or modify: broker port-forward unit tests under `crates/remote-exec-broker/src/port_forward/`

**Notes / constraints:**
- Preserve the current port-forward wire format, reconnect semantics, and broker-owned forward lifecycle.
- The `queue_or_send_tcp_connect_frame` cleanup is a safety refactor. The goal is to eliminate the pending-byte accounting foot-gun, not to change backpressure policy.
- The UDP loop cleanup should follow the already-established TCP bridge boundary pattern where practical, but avoid a forced symmetry rewrite if the smaller extraction is clearer.
- Add a regression test that proves the listen-handshake failure path actually aborts the connect tunnel rather than relying on inspection only.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker-visible TCP/UDP forwarding behavior remains green and the new abort-coverage path passes.
- Run: `cargo test -p remote-exec-broker port_forward:: -- --nocapture`
- Expect: port-forward unit coverage stays green after the internal reshaping.

- [ ] Reconfirm the current connect-tunnel abort path and the exact pending-byte/UDP epoch seams in the current broker code
- [ ] Add the missing regression test for listen-handshake failure cleanup
- [ ] Refactor `queue_or_send_tcp_connect_frame` and the UDP epoch `select!` handling into safer internal seams without changing behavior
- [ ] Run focused broker forwarding verification
- [ ] Commit

### Task 3: Close The Remaining Worthwhile Minor Fixes

**Intent:** Finish the still-actionable smaller round-3 items without reopening deferred large refactors or low-value policy churn.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely modify: `crates/remote-exec-broker/src/target/capabilities.rs`
- Likely inspect or modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/tests/`

**Notes / constraints:**
- Keep the C++ `write_stdin` work narrowly scoped to the worthwhile split and threshold-check race fix. Do not turn it into the larger `LiveSession` ownership pass.
- `clear_on_transport_error` should be narrowed only as far as the current crate boundaries allow. This is a visibility correction, not a behavior change.
- Leave `unix_timestamp_string`, warning-headroom policy, and the helper-noise items alone unless one of them naturally disappears while making an in-scope fix.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: session-store changes remain green in the POSIX daemon test suite.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec behavior and transport-error cleanup remain unchanged after the visibility narrowing.

- [ ] Reconfirm the current `write_stdin` responsibility split, threshold-check timing, and `clear_on_transport_error` visibility seam
- [ ] Split the C++ `write_stdin` path into clearer internal helpers while moving the warning-threshold check inside the lock that owns insertion
- [ ] Narrow `clear_on_transport_error` to crate-internal use and update any direct callers if needed
- [ ] Run focused C++ and broker verification
- [ ] Commit

### Task 4: Final Round 3 Sweep And Quality Gate

**Intent:** Revalidate the final tree against the reduced round-3 scope, confirm the deferred items stayed deferred intentionally, and run the final verification pass for the touched areas.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit-round-3.md`
- Likely inspect: the files touched by Tasks 1 through 3
- Existing references: `docs/superpowers/plans/2026-05-15-code-quality-audit-round-2-tier-3-plan.md`
- Existing references: `docs/superpowers/plans/2026-05-15-code-quality-audit-round-2-tier-6-remnant-plan.md`

**Notes / constraints:**
- Do not expand this sweep into the deferred `daemon_client.rs` or `LiveSession` redesigns.
- Only make a sweep follow-up edit if verification uncovers a real regression or an incomplete in-scope fix.
- Do not create an empty sweep commit.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forward coverage stays green on the final tree.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec coverage still passes after the final sweep.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the final C++ daemon tree remains green for the touched POSIX/runtime areas.

- [ ] Recheck the finished code against every in-scope round-3 item and confirm any exclusions are intentional
- [ ] Reconfirm that deferred items `3.7` and `3.8` were not accidentally mixed into the batch
- [ ] Run the final focused quality gate for the touched Rust and C++ areas
- [ ] Commit any real sweep-only follow-up if needed; otherwise do not create an empty commit
