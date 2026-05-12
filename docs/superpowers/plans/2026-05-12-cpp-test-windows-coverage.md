# C++ Test Windows Coverage Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Make the C++ daemon test suite stop assuming POSIX where behavior is platform-neutral, and expand Windows runtime coverage through the existing native-or-runner test paths.

**Requirements:**
- Preserve the current POSIX `check-posix` coverage.
- Expand Windows coverage beyond session-store and transfer tests.
- Keep Windows test execution compatible with both native Windows execution and optional runner-based execution such as Wine.
- Do not force the full POSIX `test_server_streaming` suite onto Wine in this slice.
- Keep genuinely POSIX-specific behavior explicit rather than hidden inside broad `#ifdef` blocks.
- Prefer reusable cross-platform test helpers over per-test socket/path conditionals when the abstraction is small and clear.

**Architecture:** Classify C++ tests by platform dependency and add Windows targets first for platform-neutral tests. Introduce small test-only helpers for filesystem and connected socket behavior so tests that only need byte streams can run on POSIX and Windows. Split mixed server/route coverage where needed so route-level behavior that does not depend on POSIX shells or PTYs can run in Windows test binaries, while process/PTY and heavy streaming coverage remains platform-specific.

**Verification Strategy:** Drive each expansion with a Windows build/run target first, then keep POSIX green. Use `make -C crates/remote-exec-daemon-cpp check-windows-xp` for MinGW XP binaries under Wine on Linux or natively on Windows, `make -C crates/remote-exec-daemon-cpp check-posix` for POSIX, and `nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-native` where MSVC is available.

**Assumptions / Open Questions:**
- GNU make Windows test rules already support native execution on Windows and runner-based execution elsewhere through `WINDOWS_XP_TEST_RUNNER`; preserve and reuse that pattern.
- Linux development environments may have Wine available, but CI should not require additional test-runner semantics beyond the existing makefile variable.
- MSVC XP execution may remain limited by local toolchain availability; NMake source lists should still be kept in sync with the expanded Windows-capable tests where practical.
- Full port-forward streaming under Wine is deferred unless the smaller socket/server test layer proves stable.

---

### Task 1: Classify And Name C++ Test Surfaces

**Intent:** Make it clear which C++ tests are platform-neutral, which are Windows-capable, and which remain POSIX-only.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/posix.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/NMakefile`
- Existing references: `crates/remote-exec-daemon-cpp/tests/*.cpp`

**Notes / constraints:**
- Avoid renaming every existing target in one churn-heavy pass.
- Separate source-list intent in make variables before moving substantial test code.
- Existing POSIX target names should keep working.
- Windows test commands must run directly when no runner is configured and through `WINDOWS_XP_TEST_RUNNER` when one is configured.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp -n check-windows-xp`
- Run: `make -C crates/remote-exec-daemon-cpp -n check-posix`
- Expect: dry-run output shows the same POSIX tests plus clearer Windows test targets without changing runtime behavior yet.

- [ ] Inspect current source lists and target names.
- [ ] Add or adjust make variables that identify Windows-capable test groups.
- [ ] Preserve existing focused test target names.
- [ ] Run dry-run verification.
- [ ] Commit.

### Task 2: Expand Windows Coverage For Platform-Neutral Tests

**Intent:** Add Windows XP and MSVC runtime test targets for tests that do not require POSIX process or socket behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/NMakefile`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_*.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_http_request.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_patch.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_sandbox.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_basic_mutex.cpp`

**Notes / constraints:**
- Fix test portability issues in these files instead of excluding whole test binaries when possible.
- Keep C++11 compatibility for XP-targeted tests.
- Do not add Windows-only assumptions to tests that still run on POSIX.
- Clean up unused helper warnings caused by platform-specific compiled-out sections.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: Windows XP test run includes the newly added platform-neutral binaries and POSIX remains green.

- [ ] Add Windows XP GNU make targets for platform-neutral test binaries.
- [ ] Add equivalent MSVC/NMake native targets where the source set is compatible.
- [ ] Fix any C++11 or `_WIN32` portability issues exposed by the new builds.
- [ ] Run focused and full C++ verification.
- [ ] Commit.

### Task 3: Add Cross-Platform Connected-Socket Test Helper

**Intent:** Replace POSIX-only `socketpair(AF_UNIX, ...)` assumptions in tests that only need two connected byte-stream sockets.

**Relevant files/components:**
- Likely create or modify: `crates/remote-exec-daemon-cpp/tests/test_socket_pair.h`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/NMakefile`

**Notes / constraints:**
- POSIX implementation may keep using `socketpair`.
- Windows implementation should use loopback TCP accept/connect and initialize Winsock in the helper if needed.
- Keep the helper test-only; do not move it into production socket abstractions.
- Preserve the makefile runner model: compiled Windows binaries should not care whether they run under Wine or native Windows.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-connection-manager test-host-server-transport`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Expect: connection manager and server transport coverage runs on Windows and POSIX.

- [ ] Add the test helper with POSIX and Windows implementations.
- [ ] Migrate connection-manager and server-transport tests to the helper.
- [ ] Add Windows targets for migrated tests.
- [ ] Run focused verification on POSIX and Windows XP.
- [ ] Commit.

### Task 4: Split Platform-Neutral Route Coverage From POSIX Process Route Coverage

**Intent:** Make server route tests that do not require POSIX shells, PTYs, or process behavior run on Windows.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/tests/test_server_routes_common.cpp` or a similarly scoped split file
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/posix.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/NMakefile`

**Notes / constraints:**
- Do not bury large platform differences in one file if a split keeps intent clearer.
- Platform-neutral route coverage should include request ID echo/generation, target info, auth failures, patch route behavior, transfer route validation, image route basics, and helper normalization where compatible.
- POSIX process route coverage can stay in the existing POSIX test binary or a renamed POSIX-specific route binary.
- Windows process-specific route behavior may be added only where it is stable and clearly valuable.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Expect: route coverage runs on both platforms, while POSIX-only command/PTY route coverage still runs under POSIX checks.

- [ ] Identify route assertions that are platform-neutral.
- [ ] Split or reorganize route tests without losing existing POSIX assertions.
- [ ] Add Windows targets for the platform-neutral route test binary.
- [ ] Run focused and full C++ verification.
- [ ] Commit.

### Task 5: Keep Heavy Streaming POSIX And Add A Small Windows Server Smoke If Stable

**Intent:** Avoid destabilizing CI with full Wine streaming coverage while still adding a small Windows server-level check if the socket helper supports it cleanly.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/tests/test_server_smoke.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/NMakefile`

**Notes / constraints:**
- Do not port the full `test_server_streaming` suite in this task unless it becomes obviously low-risk during implementation.
- A small smoke should avoid long reconnect timing and high-churn networking.
- If no stable small smoke exists without intrusive changes, document the deferral and leave this task as a no-code decision plus docs.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Expect: existing POSIX streaming coverage remains green; any added Windows server smoke is stable under the configured runner/native path.

- [ ] Inspect whether a minimal Windows server smoke can use existing helpers cleanly.
- [ ] Add a small Windows-capable smoke only if it is stable and low-risk.
- [ ] Keep full streaming tests POSIX-only for this slice.
- [ ] Run focused verification.
- [ ] Commit.

### Task 6: Update CI And Documentation For Expanded C++ Test Matrix

**Intent:** Make the new test split and native-or-runner Windows execution contract visible to maintainers.

**Relevant files/components:**
- Likely modify: `.github/workflows/ci.yml`
- Likely modify: `crates/remote-exec-daemon-cpp/README.md`
- Likely modify: `README.md`

**Notes / constraints:**
- CI already runs Linux POSIX and Windows XP checks in parallel and MSVC native checks on Windows; keep that shape unless the expanded targets require small naming updates.
- Documentation should name the new focused Windows test targets and clarify that `WINDOWS_XP_TEST_RUNNER` can be empty for native execution or set to Wine.
- Do not add a new CI platform unless required by the implementation.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Run where available: `nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-native`
- Expect: docs match the test matrix and CI commands remain valid.

- [ ] Update CI command names only if target names changed.
- [ ] Update README and C++ daemon README focused-test lists.
- [ ] Run final C++ verification.
- [ ] Commit.
