# Quiet Test Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Reduce normal full-suite stderr noise by quieting test-launched remote-exec subprocess logs while preserving opt-in diagnostics.

**Architecture:** Add small test-harness helpers at process boundaries. Rust broker tests set a quiet default before spawning real broker children. C++ integration and make test harnesses set quiet defaults for spawned daemon/test binaries.

**Tech Stack:** Rust integration tests with `tokio::process::Command`; GNU make and BSD make recipes for C++ POSIX tests.

---

### Task 1: Rust Broker Test Spawns

**Files:**
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target/support.rs`

**Testing approach:** existing tests + stderr measurement.
Reason: This is harness behavior with no public API change; existing integration tests exercise the spawn paths.

- [x] Add a helper that sets `REMOTE_EXEC_LOG` to `REMOTE_EXEC_TEST_LOG` or `error` unless `REMOTE_EXEC_LOG` or `RUST_LOG` is already explicit.
- [x] Apply the helper to all real broker child `Command` values in the common support spawner.
- [x] Add the equivalent helper to `multi_target/support.rs`, where that test has a separate support module.
- [x] Run `cargo test -p remote-exec-broker --test mcp_assets`.
- [x] Run `cargo test -p remote-exec-broker --test multi_target`.

### Task 2: C++ Test Process Boundaries

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Modify: `crates/remote-exec-daemon-cpp/mk/common.mk`
- Modify: `crates/remote-exec-daemon-cpp/BSDmakefile`

**Testing approach:** existing tests + stderr measurement.
Reason: C++ daemon integration and C++ host tests already cover behavior; the change only adjusts default log environment for tests.

- [x] Add a Rust helper in `mcp_forward_ports_cpp.rs` for spawned broker/C++ daemon commands.
- [x] Set C++ daemon child defaults to `REMOTE_EXEC_LOG=error`.
- [x] Set streamable HTTP broker child defaults to `REMOTE_EXEC_LOG=error`.
- [x] Update GNU make `run_test` to run host tests with a make-level `TEST_LOG_LEVEL` that selects `REMOTE_EXEC_LOG`, then `REMOTE_EXEC_TEST_LOG`, then `off`.
- [x] Update `BSDmakefile` test recipes with the same make-level default.
- [x] Run `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`.
- [x] Run `make -C crates/remote-exec-daemon-cpp check-posix`.

### Task 3: Workspace Measurement and Commit

**Files:**
- Inspect all modified files.

**Testing approach:** full verification.
Reason: The requested result is full-suite output reduction, so the final proof must measure the workspace test run.

- [x] Run `cargo fmt --all --check`.
- [x] Run `cargo test --workspace > /tmp/remote-exec-cargo-test-after.stdout 2> /tmp/remote-exec-cargo-test-after.stderr`.
- [x] Measure stderr with `wc -l` and `rg -c ' INFO | WARN '`.
- [x] Run `git diff --check`.
- [ ] Commit all task changes with message `test: quiet subprocess logs`.
