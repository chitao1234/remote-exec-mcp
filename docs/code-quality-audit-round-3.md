# Code Quality Audit — Round 3

> Analysis date: 2026-05-15. Read-only — no code was modified.
>
> Each round-2 claim was verified by reading the current source. Status reflects what is in the tree, not what commit messages assert.

---

## Round 2 Verification Matrix

| # | Issue | Status | Evidence |
|---|-------|--------|----------|
| 1.1 | Header injection via `destination_path` | FIXED | `proto/src/rpc/transfer.rs:176` — `required_transfer_header` now calls `validate_transfer_header_value` (lines 200–212). Two new tests cover `\n`/`\r\n` injection. Destination path is also base64-encoded. |
| 1.2 | `patch_engine` `.tmp` race | FIXED | `daemon-cpp/src/patch_engine.cpp:64–76` — `unique_atomic_write_temp_path` uses `<path>.tmp.<pid>.<monotonic_ms>.<atomic_counter>`. |
| 2.1 | `RpcErrorCode` wire-table drift | FIXED | `proto/src/rpc/error.rs:59–110` — single `rpc_error_code_mappings!` macro generates both `wire_value` and `from_wire_value`. Round-trip tests at 151–183. |
| 2.2 | `WARNING_THRESHOLD` hardcoded (Rust) | FIXED | `host/src/exec/store.rs:191` — `warning_threshold()` returns `self.limit.saturating_sub(WARNING_THRESHOLD_HEADROOM)`. |
| 2.3 | `WARNING_THRESHOLD` hardcoded (C++) | FIXED | `daemon-cpp/src/session_store.cpp:66` — `warning_threshold(max_open_sessions)` derives from runtime parameter. |
| 2.4 | `store_running_session` TOCTOU | FIXED | `host/src/exec/store.rs:66+` — `insert` returns `InsertOutcome { lease, … }` with the lease constructed inside the lock. `handlers.rs:283` uses `insert_outcome.lease` directly. |
| 2.5 | `active_access` double lock | FIXED | `host/src/port_forward/types.rs:16` — `TunnelState` now has a single `active: Mutex<ActiveTunnelState>`; `open_mode` is gone. |
| 2.6 | gen/tunnel non-atomic | FIXED | `broker/src/port_forward/session.rs:23–26, 82` — `generation: u64` lives inside `ListenSessionState` under one mutex; `replace_current_tunnel` updates both atomically. |
| 2.7 | `close_listen_session` `None` tunnel | FIXED | `broker/src/port_forward/supervisor/reconnect.rs:313–316` — early `return Ok(())` when `current_tunnel` is `None`. |
| 2.8 | Connect tunnel leak | FIXED | `broker/src/port_forward/supervisor/open.rs:263–267` — explicit `connect_tunnel.abort().await` on listen-handshake error. **No test covers this path.** |
| 2.9 | `BackgroundTasks::join_all` deadlock | FIXED | `host/src/state.rs:27–42` — drains `JoinSet` into local with `mem::take`, drops the lock, then awaits. Regression test at line 125. |
| 2.10 | C++ `release_counter` `assert()` | CHANGED, NOT FIXED | `daemon-cpp/src/port_tunnel.cpp:58` — replaced with `std::abort()`. This now also fires unconditionally in release builds. The underlying counter-underflow bug is not addressed; the change makes the crash observable in release, which is arguably better, but the question of how to recover gracefully is unanswered. |
| 2.11 | `release_active_tcp_stream` no RAII | PARTIALLY FIXED | `broker/src/port_forward/tcp_bridge.rs:702, 846–850` — replaced per-branch calls with `TcpActiveStreamSettlement` enum + `settle_active_tcp_stream` helper. Centralized but still manual; no scope guard. A new code path that forgets to settle still leaks. |
| 3.1 | port-forward `rpc_error` semantics | FIXED | `host/src/port_forward/error.rs:4, 8` — split into `request_error` (400) and `operational_error` (502). Unit tests at 36–48. |
| 3.2 | `daemon_client.rs` kitchen sink | PARTIALLY FIXED | File grew to 1119 lines (was 1018). All cited transfer methods (`transfer_export_to_file:430`, `transfer_export_stream:458`, `send_transfer_export_request:507`, `send_transfer_import_request:563`, `decode_transfer_import_response:591`, `open_transfer_import_body:790`) and `normalize_tool_error:805` / `normalize_tool_result:815` remain. The `TargetHandle` surface was cleaned via a new `RemoteTargetHandle<'a>` wrapper, but the transport layer is unchanged. |
| 3.3 | `TargetHandle` raw dispatch `pub` | FIXED | `broker/src/target/handle.rs` — all dispatch methods are now `pub(crate)`. |
| 3.4 | `ForwardRuntime.store` direct access | FIXED | `broker/src/port_forward/supervisor.rs:215–228` — `try_reserve_active_stream` is now a method on `ForwardRuntime`. `tcp_bridge.rs:189` calls it via the runtime. |
| 3.5 | `LiveSession` open struct | NOT FIXED | `daemon-cpp/include/live_session.h` is still `struct` with all members public. No `private:` section, no friend declarations, no accessors. Locking contract still undocumented. |
| 3.6 | `BrokerState` `pub` fields | FIXED | `broker/src/state.rs:13–20` — all eight fields are `pub(crate)`. |
| 4.1 | O(n) scan under write lock | FIXED | `broker/src/port_forward/store.rs:238` — added `reconnecting_count: usize`; `ensure_reconnect_capacity:264` reads it directly. Counter maintained by `adjust_reconnecting_count:315`. |
| 4.2 | `close_lock` across I/O | FIXED | `broker/src/port_forward/store.rs:60–84` — `_close_guard` dropped at line 63 immediately after `take_close_candidates`. Test at 648. |
| 4.3 | `wait_closed` 3× timeout | FIXED | `broker/src/port_forward/tunnel.rs:236–254` — single `tokio::time::timeout` around `tokio::join!`. Test at 710. |
| 4.4 | `DaemonConfig::validate` clones | PARTIALLY FIXED | `daemon/src/config/mod.rs:137, 201` — switched to `From<&DaemonConfig>`. Avoids the `self.clone()` at the top, but the impl still clones every field; allocation cost is unchanged. Cosmetic refactor. |
| 4.5 | Heartbeat echo blocks reader | FIXED | `broker/src/port_forward/tunnel.rs:116–135` — `try_send` with `Full → debug log + drop`, `Closed → return`. Test at 657. The reader loop body was not extracted to a named function. |
| 5.1 | C++ `overwrite_mode` stringly-typed | FIXED | `daemon-cpp/include/transfer_ops.h:21` — `enum class TransferOverwrite { … }`. |
| 5.2 | `NULL` vs `nullptr` | FIXED | `grep -n '\bNULL\b' crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/include` (excluding third_party) — zero matches. |
| 6.1 | `running_session_response` single-use | NOT FIXED | `host/src/exec/handlers.rs:307` — still defined; called twice (lines 121 and 293). Two callers does not justify the helper. |
| 6.2 | `session_limit_warnings` single-use | FIXED | Inlined per `9dcc5e6 Inline exec session warning helper`. Function no longer present. |
| 6.3 | `exec_start_request` single-use | FIXED | No top-level definition in `tools/exec.rs` (only a test name `exec_start_request_omits_none_fields`). |
| 6.4 | `forward_exec_start` single-use | FIXED | No definition found. |
| 6.5 | `apply_patch_warning` single-use | FIXED | No definition found. |
| 6.6 | `format_command_text` / `format_poll_text` | NOT FIXED | `broker/src/tools/exec_format.rs:3, 11` — both still exist as `pub(super) fn` wrappers. |
| 6.7 | `invalid_enum_header` single-use | NOT FIXED | `proto/src/rpc/transfer.rs:305` — still defined; called four times (236, 248, 278, 295). All callers are inside the same file and pass `&'static str` literals; the helper adds no narrowing. |
| 6.8 | `apply_daemon_client_timeouts` | NOT FIXED | `broker/src/daemon_client.rs:775` — still `pub(crate)`. |
| 6.9 | `ToolOperationError` single-use | (not verified) | Output truncated; treat as open. |
| 6.10 | `TcpReadLoopTarget` enum dispatch | FIXED | `host/src/port_forward/tcp.rs:37` — replaced by `trait TcpReadLoopContext`, blanket-implemented on `ConnectContext` (43) and `ListenContext` (57). Helpers at 71, 78, 95, 107, 111, 115 are now generic. |
| 6.11 | C++ `import_path` 6-overload cascade | FIXED | `daemon-cpp/include/transfer_ops.h:104, 112` — collapsed to two functions (one per archive source). |
| 7.1 | `queue_or_send_tcp_connect_frame` | NOT FIXED | `broker/src/port_forward/tcp_bridge.rs:568–631` — function is 64 lines with the same two-branch structure (`if stream_ready { … } else { … }`). The else branch's pending-budget accounting (608–627) still has the documented foot-gun: any new early return loses pending bytes. |
| 7.2 | `forward_exec_write` daemon-restart | NOT FIXED | `broker/src/tools/exec.rs:286–327` — restart-detection branch is still inline at 314–323 with no extraction or comment. |
| 7.3 | `exec_start_response` reconstructs | NOT FIXED | `broker/src/tools/exec.rs:365` — still destructures `ExecResponse::Running` and rebuilds it. |
| 7.4 | C++ `write_stdin` 103 lines | NOT FIXED | `daemon-cpp/src/session_store.cpp:427–530` — exactly 103 lines, same seven responsibilities. |
| 7.5 | UDP epoch loop 137-line `select!` | NOT FIXED | `broker/src/port_forward/udp_bridge.rs:37–187` — listen and connect arms still inline; no `handle_udp_*_tunnel_event` extraction. |
| 7.6 | `PortTunnel::from_stream_with_max_queued_bytes` | PARTIALLY FIXED | `broker/src/port_forward/tunnel.rs:53` — reader heartbeat path now non-blocking (4.5), but the reader async block was not extracted to a named function. |
| 8.1 | C++ `assert()` in tests | FIXED | `tests/test_assert.h` defines `TEST_ASSERT` → `test_assert::require()` (file/line, formatted message, unconditional `std::abort` regardless of `NDEBUG`). Zero raw `assert(` calls remain in the test tree. |
| 8.2 | `test_server_streaming.cpp` 1690-line monolith | FIXED | File reduced to 20 lines (entry + dispatch). Split into `test_server_streaming_shared.cpp` (260) + `_limits` (422) + `_routes` (340) + `_protocol` (283) + `_lifecycle` (142) + `_tcp` (132) + `_udp` (67), with named `assert_*` test functions. |
| 8.3 | Duplicated `make_config` | FIXED | Single definition in `test_server_routes_shared.h:9–23` as `make_server_routes_test_config`. |
| 8.4 | Tool-name list in three places | FIXED | `broker/src/tools/registry.rs:14–22` — single `BrokerTool::ALL`; `from_name`/`name` round-trip test at 55–63. No `NAMES` array, no `MCP_TOOL_NAMES`. |

**Summary:** 30 items verified fixed, 4 partially fixed, 8 not fixed (mostly tier 6/7 cleanup and `LiveSession` encapsulation), 1 changed disposition (`std::abort` instead of `assert`).

---

## Round 3 — New Findings

### Tier 1 — Security and Correctness

#### 3.1 Sockets are not `CLOEXEC` and leak into spawned children

**Files:**
- `daemon-cpp/src/server_transport.cpp:429` — listener `socket(...)` in `create_listener()`
- `daemon-cpp/src/server_transport.cpp:459` — `accept(listener, nullptr, nullptr)` in `accept_client()`
- `daemon-cpp/src/port_forward_socket_ops.cpp:192` — `socket(...)` in `bind_port_forward_socket()`
- `daemon-cpp/src/port_forward_socket_ops.cpp:228` — `socket(...)` in `connect_port_forward_socket()`

None of these set `SOCK_CLOEXEC` (Linux `socket` flag), use `accept4(..., SOCK_CLOEXEC)`, or follow up with `fcntl(fd, F_SETFD, FD_CLOEXEC)`. The daemon forks via `ProcessSession::launch()` and every open socket at fork time is inherited by the child shell.

Concrete consequences:
- The HTTP listener fd is inherited by every spawned command. After a daemon restart, a long-running child still holding the fd may keep the port live and prevent rebinding.
- Port-forward listener and connector fds are inherited, keeping ports bound and connections half-alive after the daemon intends to close them.
- Accepted client sockets are inherited; the server cannot detect EOF on the connection while a child holds a copy.

The pipes in `create_posix_pipe()` correctly use `pipe2(..., O_CLOEXEC)` on Linux and `fcntl(F_SETFD, FD_CLOEXEC)` on other POSIX. The signal pipe in `posix_child_reaper.cpp` uses `set_cloexec_nonblock`. The socket paths were missed.

**Fix:** Use `SOCK_CLOEXEC` on `socket()` and `accept4(SOCK_CLOEXEC)` on Linux; `fcntl(F_SETFD, FD_CLOEXEC)` immediately after `socket`/`accept` elsewhere. Scope: small.

#### 3.2 `SIGPIPE = SIG_IGN` leaks into spawned children

**File:** `daemon-cpp/src/process_session_posix.cpp:379–391` (`exec_shell_child`); set in `daemon-cpp/src/server_transport.cpp:196` (`NetworkSession::NetworkSession`).

`signal(SIGPIPE, SIG_IGN)` is set in the daemon to suppress broken-pipe deaths during HTTP writes. POSIX specifies that `SIG_IGN` survives `execve()` for signals that were ignored before exec. The child's `exec_shell_child` calls `chdir` then `execve` without resetting `SIGPIPE` to `SIG_DFL`.

Result: every shell and command spawned inherits `SIGPIPE = SIG_IGN`. Pipelines that rely on SIGPIPE for clean termination (`yes | head`, `find | head`, paginated output) will run to completion instead of stopping when the pipe closes, holding CPU and producing no observable output.

**Fix:** In `exec_shell_child`, before `execve`: `signal(SIGPIPE, SIG_DFL);`. Scope: trivial.

#### 3.3 `write_text_atomic` does not preserve file mode

**File:** `daemon-cpp/src/patch_engine.cpp:220–239`

`write_text_atomic` creates the temp file with `fopen(..., "wb")`, so the temp inherits the process umask (typically 0644 or 0600). It then renames over the original. If the original was an executable script (mode 0755), the execute bits are dropped after a patch. `transfer_ops_import.cpp` does the right thing (captures `st.st_mode` and `chmod`s after writing). The patch path does not.

**Fix:** `stat()` the original before writing; `chmod()` the temp file to match before rename. Scope: small (~5 lines).

#### 3.4 `setpgid()` return value unchecked in child fork path

**File:** `daemon-cpp/src/process_session_posix.cpp:599, 611`

The standard double-`setpgid` race-closer is in place. Neither return value is checked. If the child's `setpgid(0, 0)` fails before exec, the child stays in the daemon's process group — a subsequent `kill(-pid, SIGTERM)` from `kill_process_group` then signals the daemon itself.

**Fix:** In the child, check `setpgid(0, 0)` and `_exit(126)` on failure. The parent's call can be ignored (benign if the child already moved). Scope: small.

#### 3.5 `ioctl(TIOCSWINSZ)` return value silently discarded in PTY init

**File:** `daemon-cpp/src/process_session_posix.cpp:168`

`create_posix_pty` calls `ioctl(master.get(), TIOCSWINSZ, &size)` and ignores the result. The companion `resize_pty()` (line 452) checks the return and throws on failure. A failed initial size set is undetected; the PTY runs at the kernel default until the next explicit resize.

**Fix:** Check the return and throw, or `(void)` cast to document the deliberate discard. Scope: trivial.

#### 3.6 `/dev/null` opened without `O_CLOEXEC`

**File:** `daemon-cpp/src/process_session_posix.cpp:134`

`UniqueFd fd(open("/dev/null", O_RDONLY));` is parent-side, then `dup2`'d to fd 0 in the child. The dup2 target correctly drops `FD_CLOEXEC`. The source fd is not `O_CLOEXEC`, so an intervening `fork()` inherits it unnecessarily. Low severity, but inconsistent with the pipe paths.

**Fix:** `O_RDONLY | O_CLOEXEC`. Scope: trivial.

### Tier 2 — Layering and Encapsulation Remnants

#### 3.7 `daemon_client.rs` is still 1119 lines of mixed transport + transfer I/O

**File:** `broker/src/daemon_client.rs`

The round-2 finding stands. Methods owning file streaming, archive writing, and per-operation tracing all live on `DaemonClient`. The `RemoteTargetHandle` wrapper hides them from the public surface but does not relocate the implementation. Adding a new transfer operation still requires editing the HTTP client.

**Fix:** Move to `tools/transfer/endpoints.rs` (or a sibling `transfer_client.rs`) so `DaemonClient` is generic JSON-RPC + connection state only. Scope: medium.

#### 3.8 `LiveSession` still exposes everything publicly

**File:** `daemon-cpp/include/live_session.h`

Mutexes, condvar, `output_`, `retired`, `closing`, `pump_started`, and the platform pump thread are public; trailing-underscore naming signals intent that the language does not enforce. Code in `session_store.cpp`, `session_pump.cpp`, and `session_pump_internal.h` reaches into the fields directly.

**Fix:** Convert to `class` with private members, friend `SessionStore` and the pump internals, or at minimum document the locking contract per field. Scope: medium.

### Tier 3 — Function Size and Hidden State (still open from round 2)

| Issue | Location | Current state |
|-------|----------|---------------|
| `queue_or_send_tcp_connect_frame` budget-leak trap | `tcp_bridge.rs:568–631` (64 lines) | Same two-branch shape; `else` arm still has the documented foot-gun where a future early return inside the budget-update block leaks pending bytes. |
| `forward_exec_write` restart detection in error arm | `tools/exec.rs:286–327` | Inline `target_info` probe + cache clear at 314–323; no extraction, no comment explaining the semantics. |
| `exec_start_response` destructure-then-rebuild | `tools/exec.rs:365` | Same destructure of `ExecResponse::Running` and reconstruction. Strong signal that `ExecStartResponse` is a redundant wrapper. |
| C++ `write_stdin` 103 lines, 7 responsibilities | `session_store.cpp:427–530` | Unchanged. The completion block at 490–514 still duplicates the retire/erase/join/log pattern from `start_command`. |
| UDP epoch 137-line inline `select!` | `udp_bridge.rs:37–187` | Listen arm 58–135 (~77 lines) and connect arm 136–184 (~48 lines) still inline. The TCP bridge extracted equivalent dispatch into `handle_listen_tunnel_event`/`handle_connect_tunnel_event`; UDP did not. |

### Tier 4 — Over-Abstraction Remnants

| Helper | Location | Callers |
|--------|----------|---------|
| `running_session_response` | `host/src/exec/handlers.rs:307` | 2 (lines 121, 293) — both inside the same file. Inline cost is small. |
| `format_command_text` / `format_poll_text` | `broker/src/tools/exec_format.rs:3, 11` | Thin wrappers over a private `format_exec_text` |
| `invalid_enum_header` | `proto/src/rpc/transfer.rs:305` | 4, all in the same file, all passing `&'static str` literals. Adds no narrowing over `TransferHeaderError::invalid`. |
| `apply_daemon_client_timeouts` | `broker/src/daemon_client.rs:775` | Still `pub(crate)` despite single use. |

### Tier 5 — Smaller Findings

#### 3.9 `TcpReadLoopContext` blanket impls forward to inherent methods of the same name

**File:** `host/src/port_forward/tcp.rs:43–69`

```rust
impl TcpReadLoopContext for ConnectContext {
    fn tx(&self) -> &super::TunnelSender { ConnectContext::tx(self) }
    fn generation(&self) -> u64 { ConnectContext::generation(self) }
    fn tcp_streams(&self) -> &TcpStreamMap { ConnectContext::tcp_streams(self) }
}
```

The trait method names exactly match inherent method names. The disambiguating `ConnectContext::tx(self)` form is required because the trait method shadows the inherent one. This is correct but signals that the trait could simply be the inherent `impl` — i.e., the trait is duplicating accessors that already exist.

**Fix:** Keep the trait if other places will be generic over context; otherwise drop the trait and let the read-loop helpers take `&ConnectContext`/`&ListenContext` via two thin wrappers, or use a single struct type. Scope: small.

#### 3.10 `unix_timestamp_string` swallows clock-before-epoch as zero

**File:** `broker/src/port_forward/store.rs:259`

```rust
std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
```

If the clock is set before 1970 (rare but possible on misconfigured embedded targets), the function silently returns `Duration::ZERO`. Not a panic, but a silent bad timestamp on every record. Low severity.

**Fix:** Match on the result and either log + use `Duration::ZERO`, or return an `Option`. Scope: trivial.

#### 3.11 `crosses_warning_threshold` in C++ reads `open_sessions` outside the lock

**File:** `daemon-cpp/src/session_store.cpp:70` (used by callers)

`open_sessions` is read in the caller scope and then passed in. Between the read and the corresponding `insert`, the count may change. The warning may fire at the wrong moment or be missed entirely. Pre-existing rather than introduced by the fix, but the warning-threshold rework did not address it.

**Fix:** Compute the threshold check inside the lock that performs the insert. Scope: small.

#### 3.12 `clear_on_transport_error` is `pub` despite being an internal helper

**File:** `broker/src/target/capabilities.rs:21`

The other dispatch methods on `TargetHandle` were narrowed to `pub(crate)` but this helper kept full `pub`. No external user of the broker crate has a reason to call it.

**Fix:** `pub(crate)`. Scope: trivial.

#### 3.13 No test exercises the connect-tunnel `abort()` cleanup path

**File:** `broker/src/port_forward/supervisor/open.rs:263–267`

The fix for round-2 item 2.8 (connect-tunnel leak on listen-handshake failure) added `connect_tunnel.abort().await` inside the error arm of `wait_for_listener_ready`. Searching `open.rs` and the supervisor test files turns up no test that drives a listen-handshake failure and verifies the connect tunnel was actually torn down. The fix is correct by inspection, but a regression here would be silent.

**Fix:** Add a test that simulates listener-ready timeout/error and asserts the connect tunnel aborted. Scope: small.

#### 3.14 `WARNING_THRESHOLD_HEADROOM` is still a magic constant

**File:** `host/src/exec/store.rs:13`

The headroom value `4` is fixed. With `limit = 2`, `warning_threshold()` returns `0` (via `saturating_sub`) and the warning fires on every insert. With small configured limits the feature degrades silently.

**Fix:** Either guard against `limit < HEADROOM * 2` at validation time, or compute headroom proportionally (`limit / 16`, floored). Scope: trivial.

---

## Priority Summary for Round 3

| Severity | Items |
|----------|-------|
| Tier 1 (security / correctness) | 3.1 sockets not CLOEXEC, 3.2 SIGPIPE leaks to children, 3.3 patch loses file mode, 3.4 setpgid unchecked |
| Tier 2 (encapsulation) | 3.7 `daemon_client.rs` still kitchen sink, 3.8 `LiveSession` open struct |
| Tier 3 (function size / hidden state) | 5 carryovers from round 2 (tcp_bridge budget, forward_exec_write, exec_start_response, C++ write_stdin, UDP epoch) |
| Tier 4 (over-abstraction) | 4 single-use helpers still present |
| Tier 5 (minor) | 3.5, 3.6, 3.9–3.14 |

### Recommended order

1. **3.1** (CLOEXEC) and **3.2** (SIGPIPE) — small, mechanical, real user-visible behavior fixes.
2. **3.3** (patch loses file mode) — small, has a clear analog already implemented in the transfer path.
3. **3.4** (setpgid check) — small; closes a real "kill the daemon" foot-gun.
4. **3.13** (test for connect-tunnel abort) — small; the round-2 fix is currently unverified.
5. **3.7 / 3.8** — bigger refactors; pick one per cycle to avoid churn.
6. **Tier 3 size carryovers** — pick `udp_bridge` first (the TCP bridge already shows the pattern; symmetry has measurable maintenance value).
7. **Tier 4** — sweep in one pass.
