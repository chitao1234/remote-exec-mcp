# Section 4 Cross-Platform Conditional Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Reduce the verified section 4 cross-platform conditional-compilation density without changing the public broker-daemon contract, transport behavior, or supported platform set.

**Requirements:**
- Fix the verified section 4 items from `docs/code-quality-audit-2026-05-16.md`: `4.1`, `4.2`, and `4.3`.
- Preserve the current C++ daemon public behavior, HTTP handling, socket semantics, and broker-daemon wire format.
- Preserve C++11 and Windows XP-compatible build constraints for the standalone daemon.
- Keep `server_transport.h` and `basic_mutex.h` stable unless a narrowly-scoped private helper declaration is required for the split.
- Do not implement the auditor's proposed broad `Socket` interface or any new OO transport layer in this pass.
- Prefer existing repo conventions such as per-platform translation units (`*_posix.cpp`, `*_win32.cpp`) over new abstraction families.

**Architecture:** Treat this as a boundary cleanup rather than a redesign. On the Rust side, `remote-exec-host/src/exec/shell.rs` should become explicit about unsupported targets by failing at compile time with a clear message instead of relying on missing symbol errors. On the C++ side, split platform-specific code away from common logic: keep shared HTTP parsing, request-body streaming, and RAII flow readable in common files, and move Win32/POSIX implementation detail into platform-specific translation units or a narrow private helper header where needed. The Win32 condition-variable emulation and socket/session lifecycle behavior must remain functionally unchanged; the goal is isolation and clarity, not semantics churn.

**Verification Strategy:** Run focused Rust and C++ checks per task, then finish with the C++ POSIX and Windows XP cross-build/test paths plus Rust formatting/clippy gates that cover the touched Rust file. For the C++ transport split, use the existing host test targets and the MinGW plus Wine Windows XP path instead of compile-only confidence.

**Assumptions / Open Questions:**
- `shell.rs` should likely use a module-level `compile_error!` or equivalent unsupported-target guard, but implementation should confirm the least noisy placement once the exact module imports are visible.
- `server_transport.cpp` will probably split into one common file plus `server_transport_posix.cpp` and `server_transport_win32.cpp`, but implementation should confirm whether `create_listener` and `accept_client` stay common or move platform-side based on which choice yields less helper churn.
- Source-list churn should stay centralized in `crates/remote-exec-daemon-cpp/mk/sources.mk` where possible; touch `NMakefile` or dialect-specific makefiles only when the shared source inventory cannot express the new shape cleanly.

---

### Task 1: Add explicit compile-time platform guards and prepare shared source groups

**Intent:** Make unsupported Rust targets fail clearly, and prepare the C++ source inventory so the later platform splits land through one authoritative build-file seam instead of many scattered path edits.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/exec/shell.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Existing references: `crates/remote-exec-daemon-cpp/mk/host-tests.mk`
- Existing references: `crates/remote-exec-daemon-cpp/mk/windows-native.mk`
- Existing references: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Existing references: `crates/remote-exec-daemon-cpp/NMakefile`

**Notes / constraints:**
- The Rust change is intentionally small: add an authoritative unsupported-target path rather than inventing fallback shell behavior.
- If the C++ split can be expressed by introducing shared variables such as common/platform source groups in `sources.mk`, prefer that over editing every consumer list separately.
- Keep build entry points aligned across GNU make, BSD make, MinGW+Wine XP, and MSVC paths by changing the shared source inventory first.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Run: `cargo fmt --all --check`
- Run: `cargo clippy -p remote-exec-host --all-targets --all-features -- -D warnings`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-basic-mutex`
- Expect: supported Rust targets still compile and test cleanly, and the C++ source-list refactor does not break existing host test builds.

- [ ] Confirm the exact unsupported-target guard placement in `shell.rs`
- [ ] Add the Rust compile-time guard with a clear unsupported-platform message
- [ ] Introduce or adjust shared C++ source-group variables in `mk/sources.mk` so later platform file splits remain centralized
- [ ] Check whether `NMakefile` or the dialect-specific makefiles need only variable consumption changes, not duplicated source-list edits
- [ ] Run focused verification
- [ ] Commit

### Task 2: Split `basic_mutex` into platform translation units without changing behavior

**Intent:** Remove the per-method `#ifdef` interleaving from `basic_mutex.cpp` while preserving the current C++11 mutex and condition-variable behavior on both POSIX and Win32.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/basic_mutex.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/basic_mutex.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/basic_mutex_posix.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/basic_mutex_win32.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_basic_mutex.cpp`

**Notes / constraints:**
- Preserve the Win32 waiter/event protocol exactly, including the current `InterlockedCompareExchange(&waiters_, 0, 0)` waiter-count peek before `SetEvent`.
- Add a succinct comment near the Win32 condition-variable emulation explaining the waiter-count and broadcast-reset protocol so the behavior is easier to audit later.
- Keep `BasicLockGuard` in a common location if that avoids needless duplication; the goal is to isolate platform behavior, not maximize file count.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-basic-mutex`
- Run: `make -C crates/remote-exec-daemon-cpp test-windows-xp-basic-mutex`
- Run: `make -C crates/remote-exec-daemon-cpp test-msvc-native-basic-mutex` when an MSVC environment is available during execution
- Expect: POSIX and Windows basic-mutex tests remain green with no behavior changes.

- [ ] Confirm whether any common `BasicLockGuard` or shared helper code should remain in `basic_mutex.cpp`
- [ ] Move POSIX and Win32 mutex/condvar implementations into dedicated translation units
- [ ] Preserve and document the Win32 waiter/event protocol without changing its semantics
- [ ] Update the shared source inventory and any directly affected test source groups
- [ ] Run focused verification
- [ ] Commit

### Task 3: Split `server_transport` platform helpers and finish with a section 4 sweep

**Intent:** Separate the dense platform-specific socket/session code from the shared HTTP transport logic while keeping the public transport surface, error classification, and runtime behavior intact.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/server_transport.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/server_transport_posix.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/server_transport_win32.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/server_transport_internal.h` or similar private helper header if required
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Existing references: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/win32_error.cpp`

**Notes / constraints:**
- Keep the flat helper style already exposed by `server_transport.h`; do not replace it with a new `Socket` class hierarchy.
- The common file should stay responsible for shared HTTP framing, request-body streaming, `UniqueSocket`, and other platform-neutral flow that reads better without preprocessor noise.
- Move platform-specific socket close/shutdown behavior, timeout application, last-error handling, WSA startup/cleanup, and any unavoidable cloexec differences behind the split.
- If `create_listener` and `accept_client` remain partially shared, the remaining helper indirection must stay small and obvious. If duplicating them across platform files is cleaner, that is acceptable so long as the duplication is bounded and behavior-preserving.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-connection-manager`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Run: `cargo test -p remote-exec-host`
- Run: `cargo fmt --all --check`
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: section 4 items are resolved, the C++ POSIX and XP build/test paths stay green, and the Rust guard change remains clean under formatting and lint gates.

- [ ] Confirm the exact common versus platform split for `server_transport`, including whether a small private helper header is warranted
- [ ] Move the platform-specific socket/session helpers into dedicated POSIX and Win32 translation units while keeping the public header stable
- [ ] Keep shared HTTP and RAII logic in the common file, trimming conditional-compilation density materially rather than cosmetically
- [ ] Update or extend focused transport tests only where the new file boundaries require it
- [ ] Run focused and final verification for the full section 4 scope
- [ ] Summarize any intentionally retained tiny `#ifdef` holdouts that remain justified after the split
- [ ] Commit
