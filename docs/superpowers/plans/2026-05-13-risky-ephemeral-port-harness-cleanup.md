# Risky Ephemeral Port Harness Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Remove the repo's flaky "bind `127.0.0.1:0`, drop the listener, then expect the address to work later" test harness pattern without changing normal runtime behavior or touching legitimate live `:0` listeners that keep ownership of the socket.

**Requirements:**
- Purge only the risky bind-read-drop-reuse pattern, not every use of `127.0.0.1:0`.
- Preserve real broker-process coverage for streamable-HTTP broker tests, including the C++ daemon integration path.
- Preserve daemon runtime behavior and public broker/daemon contracts; this is a test harness cleanup, not a product feature change.
- Replace "dead target" placeholders with deterministic negative fixtures that do not rely on a guessed free port remaining unused.
- Keep plan-based execution and commit after each real implementation batch only; do not create empty commits.

**Architecture:** Treat the risky seams as a harness-ownership problem. For spawned broker processes, stop guessing a future port and instead discover the actual bound address after the broker binds, using the broker's own startup output in test support. For daemon integration support, prebind listeners in the test harness and pass the live listener into the daemon runtime using the existing listener-owned seam. For intentionally unavailable targets, use listener-owned negative fixtures instead of dropped "probably free" ports.

**Verification Strategy:** Verify each harness batch with the narrowest tests that actually exercise the changed ownership seam, then widen to the full broker and daemon integration targets that depend on those helpers. Broker streamable-HTTP changes should prove both ordinary MCP HTTP startup and the real C++ daemon path. Negative-target cleanup should prove cached-unavailable target behavior and healthy-target isolation. Daemon listener ownership changes should prove plain HTTP and TLS startup helpers without relying on startup bind retries.

**Assumptions / Open Questions:**
- The broker startup log already includes the post-bind listen address; implementation should confirm the exact parseable line and avoid depending on fragile incidental formatting beyond the stable `listen = ...` field.
- The dead-target replacement should fail quickly and deterministically. The preferred direction is a tiny negative HTTP fixture that accepts and closes or otherwise produces an immediate invalid daemon response, but the exact helper shape should be confirmed during implementation.
- The existing daemon test listener seam in `remote-exec-daemon::test_support` is sufficient to eliminate `reserve_listen_addr()`; implementation should confirm whether any remaining startup retry loop still has value once the bind race is gone.

---

### Task 1: Replace Broker Streamable-HTTP Free-Port Reservation

**Intent:** Remove bind-drop-reuse from broker streamable-HTTP fixtures while preserving real spawned broker-process coverage.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Likely modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Existing references: `crates/remote-exec-broker/src/mcp_server.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_http.rs`

**Notes / constraints:**
- Keep the broker running as a real child process for these harnesses.
- Replace `allocate_unused_loopback_addr()` only where it feeds a later broker bind; do not rewrite normal live test listeners that stay open.
- The replacement harness should discover the actual bound address only after the broker has bound it.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_http`
- Expect: streamable-HTTP broker startup still becomes reachable with no guessed free-port handoff.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: the real C++ daemon broker path still passes, including the broker crash/reopen case that is timing-sensitive today.

- [ ] Confirm the exact streamable-HTTP broker fixtures that still use free-port reservation before child startup
- [ ] Add or update shared test harness logic to discover the broker's actual bound address after bind
- [ ] Remove the bind-drop-reuse helpers from those broker fixtures and wire them to the post-bind discovery path
- [ ] Run focused broker verification for HTTP and real C++ integration coverage
- [ ] Commit

### Task 2: Replace Dead-Target Placeholder Free Ports With Deterministic Negative Fixtures

**Intent:** Eliminate dropped free-port placeholders used to represent unavailable broker targets.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_exec/verification.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_assets.rs`

**Notes / constraints:**
- Do not regress the meaning of these tests: one target must remain healthy while another stays unavailable or invalid.
- Prefer a listener-owned negative fixture over a guessed dead port, so failures become structural rather than timing-based.
- Keep the existing delayed-target listener path when it already owns the listener throughout the test.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: healthy-target and wrong-daemon verification tests still pass with deterministic unavailable-target behavior.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: cached daemon-info reporting for available vs unavailable targets still passes without free-port guessing.

- [ ] Confirm the dead-target helper call sites and the exact failure behavior they need to preserve
- [ ] Add or update a deterministic negative-target fixture that owns its listener lifecycle
- [ ] Replace the dropped free-port placeholders in the broker test support helpers
- [ ] Run focused broker verification for exec verification and asset metadata coverage
- [ ] Commit

### Task 3: Convert Daemon Integration Fixtures To Listener-Owned Startup

**Intent:** Remove the daemon test support pattern that reserves a future port by dropping a temporary listener before startup.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/tests/support/spawn.rs`
- Likely modify: `crates/remote-exec-daemon/tests/support/spawn_tls.rs`
- Existing references: `crates/remote-exec-daemon/src/test_support.rs`
- Existing references: `crates/remote-exec-daemon/src/lib.rs`

**Notes / constraints:**
- Use the existing listener-owned runtime seam rather than reintroducing a new startup side channel.
- Preserve plain HTTP and TLS daemon fixture behavior, including request clients and shutdown behavior.
- Revisit startup bind retry loops after the race source is removed; keep retries only if they still protect a different, real startup failure mode.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: daemon startup helpers still support plain HTTP startup and auth/TLS-related health checks.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: daemon fixture startup used by exec integration coverage still remains stable after listener ownership changes.

- [ ] Confirm the current daemon startup helper flow and where a live listener can be injected cleanly
- [ ] Replace `reserve_listen_addr()` and related bind-drop-reuse startup paths with listener-owned startup
- [ ] Trim or remove startup bind-retry logic if it only existed to survive the old free-port race
- [ ] Run focused daemon verification for health and exec startup-dependent coverage
- [ ] Commit

### Task 4: Final Sweep For Remaining Risky Bind-Drop-Reuse Seams

**Intent:** Confirm that the risky pattern is gone repo-wide while leaving legitimate live `:0` listeners intact.

**Relevant files/components:**
- Likely inspect: `crates/remote-exec-broker/tests/`
- Likely inspect: `crates/remote-exec-daemon/tests/`
- Likely inspect: `tests/`

**Notes / constraints:**
- The final sweep is specifically about the risky ownership pattern, not a blanket ban on `127.0.0.1:0`.
- Residual live `TcpListener::bind("127.0.0.1:0")` uses are acceptable when the test continues to own the listener.

**Verification:**
- Run: `rg -n "std::net::TcpListener::bind\\(\"127\\.0\\.0\\.1:0\"\\)|TcpListener::bind\\(\"127\\.0\\.0\\.1:0\"\\)|allocate_unused_loopback_addr|reserve_listen_addr" crates tests -g '!target'`
- Expect: no remaining helper that binds `:0`, drops the socket, and hands the address to a later process or server start.
- Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- Expect: the multi-target broker harness remains stable after the streamable-HTTP and negative-target fixture cleanup.

- [ ] Audit the repo for any remaining bind-drop-reuse helper or equivalent pattern
- [ ] Clean up leftover helper names or comments that still imply the old free-port reservation model
- [ ] Run the final grep-based sweep and one cross-cutting broker integration target
- [ ] Commit
