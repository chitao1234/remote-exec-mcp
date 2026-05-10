# Port Forward Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Fix all still-valid findings from review items #11 through #16 while keeping the stale #10 TCP write-lock finding unchanged.

**Architecture:** Rust host stream state will move from separate writer/permit/cancel maps into typed TCP and UDP entries guarded by one mutex per state owner. C++ session expiry and shutdown drains will use existing XP-compatible `BasicMutex`/`BasicCondVar` primitives instead of detached sleeper threads or repeated busy sleeps.

**Tech Stack:** Rust 2024 with Tokio, C++ daemon with POSIX and Windows XP-compatible threading primitives, existing port-tunnel and daemon C++ test harnesses.

---

### Task 1: Save The Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-10-port-forward-review-fixes.md`
- Test/Verify: `git status --short`

**Testing approach:** no new tests needed
Reason: This is a planning artifact only.

- [x] **Step 1: Add this plan file.**

- [ ] **Step 2: Commit the plan.**

Run: `git add docs/superpowers/plans/2026-05-10-port-forward-review-fixes.md && git commit -m "docs: plan port forward review fixes"`

### Task 2: Fold Rust TCP And UDP State

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Test/Verify: `cargo test -p remote-exec-host port_tunnel_tests`

**Testing approach:** existing tests + targeted verification
Reason: Existing port-tunnel tests cover EOF, close, retained UDP pressure, reconnect, and cleanup behavior.

- [ ] **Step 1: Replace transport-owned `tcp_writers`, `tcp_stream_permits`, `udp_sockets`, and `stream_cancels` with `tcp_streams` and `udp_binds` typed maps.**

- [ ] **Step 2: Replace session-owned attachment maps with `tcp_streams` and `udp_readers` typed maps.**

- [ ] **Step 3: Update TCP register, lookup, EOF, cleanup, and close paths to insert/remove one `TcpStreamEntry`.**

- [ ] **Step 4: Update UDP bind, retained read-loop activation, datagram lookup, cleanup, and close paths to insert/remove one `UdpBindEntry` or `UdpReaderEntry`.**

- [ ] **Step 5: Run targeted verification.**

Run: `cargo test -p remote-exec-host port_tunnel_tests`

- [ ] **Step 6: Commit.**

Run: `git add crates/remote-exec-host/src/port_forward && git commit -m "refactor: fold rust port forward stream state"`

### Task 3: Replace C++ Expiry Sleeper Threads

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-server-streaming`

**Testing approach:** characterization/integration test
Reason: Expiry behavior is observable through reconnect and worker budget behavior.

- [ ] **Step 1: Add one service-owned expiry scheduler thread, condition variable, shutdown flag, and pending-session list.**

- [ ] **Step 2: Make `detach_session` enqueue a session deadline and notify the scheduler instead of spawning one sleep thread.**

- [ ] **Step 3: Ensure service destruction stops and joins the scheduler thread.**

- [ ] **Step 4: Add a test that detached sessions do not consume port-tunnel worker budget while waiting to expire.**

- [ ] **Step 5: Run targeted verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-server-streaming`

- [ ] **Step 6: Commit.**

Run: `git add crates/remote-exec-daemon-cpp && git commit -m "fix: schedule cpp port tunnel expiry centrally"`

### Task 4: Centralize C++ Shutdown Drain

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/connection_manager.h`
- Modify: `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-connection-manager test-server-runtime`

**Testing approach:** existing tests + targeted verification
Reason: The change is structural but affects shutdown blocking behavior already covered by daemon tests.

- [ ] **Step 1: Add `ConnectionManager::wait_for_all()` backed by a condition variable signaled when worker count changes.**

- [ ] **Step 2: Replace destructor and server runtime busy-poll loops with `wait_for_all()`.**

- [ ] **Step 3: Run targeted verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-connection-manager test-server-runtime`

- [ ] **Step 4: Commit.**

Run: `git add crates/remote-exec-daemon-cpp && git commit -m "fix: centralize cpp connection shutdown wait"`

### Task 5: Clarify Broker Listen Session State Semantics

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** existing tests + targeted verification
Reason: This is a maintainability and lock-order fix with existing public behavior coverage.

- [ ] **Step 1: Replace external direct `current_tunnel` access with helper methods that document and enforce the operation lock protocol.**

- [ ] **Step 2: Run targeted verification.**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`

- [ ] **Step 3: Commit.**

Run: `git add crates/remote-exec-broker/src/port_forward/supervisor.rs && git commit -m "refactor: clarify listen tunnel state locking"`

### Task 6: Handle WinPTY Mutex Poisoning

**Files:**
- Modify: `crates/remote-exec-host/src/exec/winpty.rs`
- Test/Verify: `cargo check -p remote-exec-host`

**Testing approach:** existing tests + targeted verification
Reason: The code only builds on Windows with WinPTY at runtime, so this task maps poison to errors and verifies compilation.

- [ ] **Step 1: Replace `.lock().unwrap()` in WinPTY background and public methods with poison-aware helpers.**

- [ ] **Step 2: Run targeted verification.**

Run: `cargo check -p remote-exec-host`

- [ ] **Step 3: Commit.**

Run: `git add crates/remote-exec-host/src/exec/winpty.rs && git commit -m "fix: handle winpty mutex poisoning"`

### Task 7: Final Verification

**Files:**
- Test/Verify: Rust host, broker port-forward, and C++ POSIX checks

**Testing approach:** existing tests + targeted verification
Reason: The changes span Rust host, broker supervisor, and C++ daemon concurrency.

- [ ] **Step 1: Run final focused verification.**

Run:
`cargo test -p remote-exec-host port_tunnel_tests`
`cargo test -p remote-exec-broker --test mcp_forward_ports`
`cargo check -p remote-exec-host -p remote-exec-broker`
`make -C crates/remote-exec-daemon-cpp check-posix`
