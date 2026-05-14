# Code Quality Audit — Round 2

> Analysis date: 2026-05-14. Read-only — no code was modified.
>
> Prior audit at `docs/code-quality-audit.md` addressed the bulk of duplication, structural, and dependency findings. This second pass focuses on correctness, layering, and over-abstraction introduced or left after the recent refactoring sweep.

---

## Tier 1 — Security and Data Corruption

### 1.1 HTTP header injection via `destination_path`

**File:** `crates/remote-exec-proto/src/rpc/transfer.rs:171–197`

`optional_transfer_header` rejects values containing `\n` or `\r`. `required_transfer_header` does not. The `destination_path` header is built through `required_transfer_header` and is therefore never validated for newlines. A path like `/tmp/foo\nX-Injected: header` passes validation and is forwarded as an HTTP header.

**Why it matters:** The destination path is user-controlled and lands in an HTTP request header (`x-remote-exec-destination-path`). This is a CRLF injection primitive.

**Fix:** Apply the same newline check in `required_transfer_header`, or consolidate both into a single `validated_header` function. Scope: small.

### 1.2 `write_text_atomic` uses a fixed `.tmp` suffix — concurrent patches race

**File:** `crates/remote-exec-daemon-cpp/src/patch_engine.cpp:188–203`

```cpp
const std::string temp_path = path + ".tmp";
```

Two concurrent patch operations on the same file (the HTTP server is multi-threaded) both write to `path + ".tmp"`. One clobbers the other; the rename produces a corrupted result or a spurious error. There is no per-file lock around patch operations.

**Fix:** Generate a unique temp name (`mkstemp` on POSIX, `getpid() + monotonic_ms()` suffix elsewhere). Scope: small.

---

## Tier 2 — Correctness

### 2.1 `RpcErrorCode::wire_value` duplicates the wire table

**File:** `crates/remote-exec-proto/src/rpc/error.rs:61–216`

`RPC_ERROR_CODE_WIRE_VALUES` (used by `from_wire_value`) and the `wire_value()` `match` (44 variants) carry the same mapping in two forms. Add a variant, forget to update one of them, and `from_wire_value(code.wire_value())` silently returns `None`. Existing tests spot-check only two variants.

**Fix:** Drop the table and implement `from_wire_value` as a `match` on the string, or derive the table from the match via a `const fn` / macro. Scope: small.

### 2.2 `WARNING_THRESHOLD` is hardcoded to the default limit, not the configured one

**Two locations:**
- Rust: `crates/remote-exec-host/src/exec/store.rs:185–188` — `crosses_warning_threshold` compares against `WARNING_THRESHOLD` (a file-scope constant derived from `DEFAULT_SESSION_LIMIT`), not `self.limit`.
- C++: `crates/remote-exec-daemon-cpp/src/session_store.cpp:40` — `WARNING_THRESHOLD = DEFAULT_MAX_OPEN_SESSIONS - WARNING_THRESHOLD_HEADROOM` at file scope.

If an operator sets `max_open_sessions` to a non-default value (e.g., 20), the warning fires at 60, which is never reached — the warning is silently dead. For larger configured limits, it fires far too early.

**Fix:** Compute the threshold from the configured limit at construction or call time: `self.limit.saturating_sub(WARNING_THRESHOLD_HEADROOM)`. Scope: trivial (both languages).

### 2.3 `store_running_session` inserts then re-locks the session (TOCTOU)

**File:** `crates/remote-exec-host/src/exec/handlers.rs:272–301`

```rust
let insert_outcome = state.sessions.insert(daemon_session_id.clone(), session).await;
// ...
let session = state.sessions.lock(&daemon_session_id).await
    .ok_or_else(|| internal_error(...))?;
```

Between `insert` and `lock`, `prune_for_insert` could theoretically evict the just-inserted session if the store is at capacity. The `ok_or_else` masks the race as an internal error.

**Fix:** Change `SessionStore::insert` to return a `SessionLease` directly. Scope: small.

### 2.4 `active_access` reads `open_mode` and `active` in two separate critical sections

**File:** `crates/remote-exec-host/src/port_forward/active.rs:292–306`

`tunnel_mode` locks `open_mode`; `current_connect_context` / `current_listen_context` then lock `active`. The two are always written together but read non-atomically. The error path covers the inconsistency window, but the design invites future bugs.

**Fix:** Combine `open_mode` and `active` into a single `Mutex<TunnelActiveState>` so they are atomic. Scope: medium.

### 2.5 `ListenSessionControl` generation and tunnel updated non-atomically

**File:** `crates/remote-exec-broker/src/port_forward/session.rs:71–77`

```rust
self.generation.store(generation, Ordering::Release);   // atomic
self.with_session_state(|state| {
    state.current_tunnel = Some(tunnel);                // mutex
}).await;
```

A reader that calls `current_generation()` then `current_tunnel()` sees a torn state where the generation has advanced but the tunnel has not yet been replaced.

**Fix:** Move `generation` inside `ListenSessionState` so both are guarded by the same mutex. Scope: small.

### 2.6 `close_listen_session` treats a `None` tunnel as "reconnect to close"

**File:** `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs:310–337`

When `current_tunnel` is `None`, the function falls through to `resume_listen_session_inner`, attempting to re-establish a network session purely to send a close frame. In tests `new_for_test(..., None)` produces such records, and a real-world path that ends with a never-fully-established session would trigger an unnecessary reconnect.

**Fix:** Return `Ok(())` immediately when `current_tunnel` is `None`. Scope: small.

### 2.7 Connect tunnel leaks when listen-handshake fails

**File:** `crates/remote-exec-broker/src/port_forward/supervisor/open.rs:207–314`

`build_opened_forward` opens both tunnels, then waits for the listener-ready ack. If the ack fails (timeout, error), the already-opened connect tunnel is dropped without an explicit `abort()`. The remote side holds the connection open until its own heartbeat times out.

**Fix:** Either abort the connect tunnel on error, or restructure so the connect tunnel is not opened until after the listen handshake succeeds. Scope: small.

### 2.8 `BackgroundTasks::join_all` holds the lock across all task joins

**File:** `crates/remote-exec-host/src/state.rs:30–37`

`join_next().await` is called inside the `JoinSet` mutex guard. Any background task that itself spawns another task during shutdown deadlocks.

**Fix:** Take the lock, drain handles into a local `Vec`, release, then await. Scope: small.

### 2.9 `release_counter` uses `assert()` after a log

**File:** `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp:58`

```cpp
log_message(LOG_ERROR, ...);
assert(false && "port-tunnel counter released below zero");
```

In release builds with `NDEBUG`, the `assert` is a no-op; the underflow continues silently after the log. In debug builds it aborts the whole daemon instead of failing the single request.

**Fix:** Replace with `std::abort()` if truly fatal, or throw an internal error. Scope: small.

### 2.10 `release_active_tcp_stream` is called from multiple error paths without RAII

**File:** `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:730–778`

`close_tcp_pair_if_fully_eof` calls `release_active_tcp_stream` from three separate branches. A future refactor that adds another early return before line 753 would leak the active stream count, eventually starving the forward.

**Fix:** Wrap the release in a scope-guard pattern or restructure the function so the release is unconditional. Scope: small.

---

## Tier 3 — Layering and Encapsulation

### 3.1 `port_forward/error.rs` aliases `logged_bad_request` as `rpc_error`

**File:** `crates/remote-exec-host/src/port_forward/error.rs:4`

```rust
pub(super) use crate::error::logged_bad_request as rpc_error;
```

Every call site in the port-forward module silently logs a `warn!` and returns HTTP 400 — but `PortConnectFailed`, `PortAcceptFailed`, `PortReadFailed`, `PortWriteFailed` are infrastructure failures, not bad requests. Operators see "bad request" noise for normal network failures; clients receiving 400 may treat retryable errors as permanent.

**Fix:** Remove the alias. Use `crate::error::rpc_error(status, code, message)` directly with appropriate status per call site. Scope: medium (~15 call sites).

### 3.2 `daemon_client.rs` mixes transport with transfer I/O and tool-layer policy

**File:** `crates/remote-exec-broker/src/daemon_client.rs`

- **3.2a** Lines 261–403: transfer-specific helpers (`transfer_export_to_file`, `transfer_export_stream`, `write_transfer_export_archive`, `send_transfer_export_request`, `transfer_export_metadata`, `send_transfer_import_request`, `decode_transfer_import_response`, `open_transfer_import_body`) live inside the HTTP client. They contain file I/O, streaming, and per-operation tracing that has nothing to do with generic RPC.
- **3.2b** Lines 704–719: `normalize_tool_error` / `normalize_tool_result` downcast `anyhow::Error` back to `DaemonClientError` and apply `RpcToolErrorMode`. They are called from the tool layer. Tool-layer error presentation policy does not belong in the transport.

**Fix:** Move transfer helpers into `tools/transfer/endpoints.rs` (or a sibling `transfer_client.rs`). Move the normalize helpers into a tools-side error module. Scope: medium.

### 3.3 `TargetHandle` raw dispatch methods are `pub`, bypassing checked variants

**File:** `crates/remote-exec-broker/src/target/handle.rs:98–206`

Raw methods (`exec_start`, `exec_write`, `patch_apply`, `image_read`, `transfer_path_info`, `port_tunnel`, …) are `pub`. The intended public API for tool handlers is the `_checked` variants in `capabilities.rs`. Inconsistency already exists in `tools/exec.rs`: `target.target_info()` (line 342) is called raw on purpose, but `exec_write` (line 322) is also called raw before being wrapped in `clear_on_transport_error`.

**Fix:** Make raw dispatch `pub(crate)` (or `pub(super)`) and ensure every tool handler uses the checked variant. Scope: medium.

### 3.4 `ForwardRuntime.store` accessed directly from `tcp_bridge.rs`

**File:** `crates/remote-exec-broker/src/port_forward/supervisor.rs:79`, used at `tcp_bridge.rs:806`

All store mutations in the bridges go through `ForwardRuntime` methods except `try_reserve_active_tcp_stream`, which reaches into `runtime.store` directly because it also needs `runtime.limits.max_active_tcp_streams`. The store's representation leaks into the bridge.

**Fix:** Move `try_reserve_active_tcp_stream` onto `ForwardRuntime` itself. Scope: small.

### 3.5 `LiveSession` exposes all members publicly

**File:** `crates/remote-exec-daemon-cpp/include/live_session.h`

Both mutexes, the condition variable, `output_`, `retired`, `closing`, `pump_started`, and the platform-specific `pump_thread_` are public. The trailing-underscore naming signals private intent, but the `struct` is fully open. Code in `session_store.cpp`, `session_pump.cpp`, and `session_pump_internal.h` reaches directly into the fields. The locking contract on each field is undocumented.

**Fix:** Convert to a `class` with private members and friend declarations for `SessionStore` and the pump internals, or at minimum document the locking contract per field. Scope: medium.

### 3.6 `BrokerState` fields are all `pub`

**File:** `crates/remote-exec-broker/src/state.rs:12–21`

All eight fields are `pub`. Callers outside the broker crate interact through `RemoteExecClient`, not `BrokerState`, so the outer-`pub` is unused while the field-level `pub` is a maintenance hazard.

**Fix:** Make fields `pub(crate)`; add accessors only for fields genuinely needed externally. Scope: small.

---

## Tier 4 — Lock Contention and Hot-Path Performance

### 4.1 `ensure_reconnect_capacity` scans all entries under the write lock

**File:** `crates/remote-exec-broker/src/port_forward/store.rs:216–245`

`derive_phase` is called for every entry on every reconnect attempt, holding the write lock. At high forward counts and during network instability this blocks the data-plane `update_entry` calls in `tcp_bridge.rs` / `udp_bridge.rs`.

**Fix:** Maintain a `reconnecting_count: usize` field on `PortForwardStore`, incremented/decremented on phase transitions. Check becomes O(1). Scope: small.

### 4.2 `PortForwardStore::close` holds `close_lock` across async network I/O

**File:** `crates/remote-exec-broker/src/port_forward/store.rs:60–74`

`close_handle` performs `close_listen_session`, which sends frames and waits for acks. The `close_lock` is held across the whole batch. A single hung tunnel stalls the entire close call for `LISTEN_CLOSE_ACK_TIMEOUT`, even for unrelated forwards.

**Fix:** Use the lock only to remove entries from the map atomically; perform the close work outside the lock, ideally in parallel. Scope: medium.

### 4.3 `PortTunnel::wait_closed` applies its timeout three times sequentially

**File:** `crates/remote-exec-broker/src/port_forward/tunnel.rs:227–247`

The reader, writer, and heartbeat task locks are each acquired in sequence and each receives the full `FORWARD_TASK_STOP_TIMEOUT`. Worst-case wait is 3× the configured timeout, which can cascade into `close_listen_session` timing out.

**Fix:** Join the three tasks concurrently (`tokio::try_join!`) under a single timeout. Scope: small.

### 4.4 `DaemonConfig::validate` clones the whole config

**File:** `crates/remote-exec-daemon/src/config/mod.rs:138`

```rust
HostRuntimeConfig::from(self.clone()).validate()?;
```

`From<DaemonConfig> for HostRuntimeConfig` moves all fields, so validation requires cloning the entire `DaemonConfig` (including the `ProcessEnvironment` `HashMap`). `into_validated` therefore clones twice during startup.

**Fix:** Refactor `HostRuntimeConfig::validate` to operate on a borrowed view, or extract a free `validate_fields(...)` function. Scope: small.

### 4.5 Reader task heartbeat echo can block under write pressure

**File:** `crates/remote-exec-broker/src/port_forward/tunnel.rs:116–128`

The reader task echoes heartbeat requests by `send().await` on `reader_tx` (a clone of the write-side `tx`, capacity 128). If the writer is backed up, the reader blocks awaiting capacity and stops draining the wire, which can fire a heartbeat timeout on the peer and trigger a reconnect.

**Fix:** Use `try_send` for the heartbeat echo; drop the echo on full queue (acks are best-effort). Scope: small (also extract the reader loop into a named function while there — it is currently a 56-line inline async block).

---

## Tier 5 — Type Safety and Stringly-Typed Values

### 5.1 C++ `overwrite_mode` is a raw string at every boundary

**File:** `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp:275–307`

`"fail"` / `"merge"` / `"replace"` is compared five times in `prepare_destination_path` alone, and the same strings reappear in `transfer_http_codec.cpp`. Typos fall through to a runtime "unsupported overwrite mode" error. Internal callers and tests bypass the HTTP layer's validation entirely.

**Fix:** Introduce `enum class OverwriteMode { Fail, Merge, Replace }` in `transfer_ops_internal.h`; convert at the HTTP boundary. Scope: medium.

### 5.2 `NULL` used throughout C++ instead of `nullptr`

**Files:** all production C++ files (4–16 occurrences each). The codebase already uses `std::unique_ptr`, lambdas, `= delete`, `std::atomic` — `nullptr` is universally available.

**Fix:** Mechanical replacement. Scope: small but touches many files.

---

## Tier 6 — Over-Abstraction and Single-Use Helpers

Recent refactoring extracted helpers that have only one caller and no clarifying value. Each of these is a candidate for inlining.

| Helper | File | Lines |
|--------|------|-------|
| `running_session_response` | `host/exec/handlers.rs` | 303–319 |
| `session_limit_warnings` | `host/exec/handlers.rs` | 321–330 |
| `exec_start_request` | `broker/tools/exec.rs` | 240–250 |
| `forward_exec_start` | `broker/tools/exec.rs` | 252–260 |
| `apply_patch_warning` | `broker/tools/exec.rs` | 450–452 |
| `format_command_text` / `format_poll_text` | `broker/tools/exec_format.rs` | 3–17 |
| `invalid_enum_header` | `proto/rpc/transfer.rs` | 273–275 |
| `apply_daemon_client_timeouts` | `broker/daemon_client.rs` | 674 |

### 6.1 `ToolOperationError` used in exactly one place

**File:** `crates/remote-exec-broker/src/tools/exec.rs:18–30` (constructed once at line 146)

A `thiserror` struct that wraps an error with a tool-name prefix. Only `write_stdin` uses it; `exec_command` returns raw `anyhow::Error` for its failures. The type adds no value over `anyhow!("write_stdin failed: {err}")`.

**Fix:** Remove, inline the message. Scope: small.

### 6.2 `TcpReadLoopTarget` is a hand-rolled trait

**File:** `crates/remote-exec-host/src/port_forward/tcp.rs:37–103`

Two-variant enum (`Connect`, `Listen`); six methods, each a two-arm match delegating to identically-named methods on the inner contexts. Adding a third role requires touching the enum, six match arms, and two public wrappers.

**Fix:** Replace with a `TcpReadLoopContext` trait implemented on both contexts; `tunnel_tcp_read_loop` becomes generic. Scope: small.

### 6.3 C++ `import_path` six-overload cascade

**File:** `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp:500–617`

Three overloads each for `import_path` and `import_path_from_reader`, each adding one optional parameter with a default. The 6-param and 7-param overloads exist only to supply default `TransferLimitConfig` and `TransferPathAuthorizer` values.

**Fix:** Collapse to two functions with C++11 default arguments. Scope: small.

---

## Tier 7 — Function Size and Hidden State Machines

### 7.1 `queue_or_send_tcp_connect_frame` — two-branch state machine with a budget-leak trap

**File:** `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:551–610`

The function combines "send to ready stream" and "buffer pending bytes" in one body with a top-level `if/else`. The `else` branch's budget accounting (lines 593–606) is correct only because `remove_stream_entry` zeroes `stream.pending_bytes` before subtracting. Any new early return inside that branch silently leaks the pending-bytes budget.

**Fix:** Extract `buffer_pending_tcp_connect_frame`. Make the budget invariant explicit. Scope: small.

### 7.2 `forward_exec_write` buries daemon-restart detection in an error arm

**File:** `crates/remote-exec-broker/src/tools/exec.rs:333–353`

A three-arm match handles success, `UnknownSession`, and a generic-error path that makes a second `target_info` call to detect daemon restart. The semantic decision "treat transport error as restart if instance_id changed" is hidden inside an error arm with no comment.

**Fix:** Extract `detect_daemon_restart_and_clear_session` with a doc comment. Scope: small.

### 7.3 `exec_start_response` destructures and immediately reconstructs `ExecResponse::Running`

**File:** `crates/remote-exec-broker/src/tools/exec.rs:392–408`

The destructure-reconstruct pattern signals that `ExecStartResponse` is an unnecessary wrapper.

**Fix:** Make `ExecStartResponse` hold the `ExecRunningResponse` directly, or extract `daemon_session_id` without rebuilding the response. Scope: small.

### 7.4 C++ `write_stdin` — 103 lines, seven responsibilities

**File:** `crates/remote-exec-daemon-cpp/src/session_store.cpp:422–525`

Session lookup, touch-order update, logging, PTY resize, stdin write, error translation, polling, completion path (retire + erase + join + log), running path. The completion logic duplicates a similar block in `start_command` (lines 485–509).

**Fix:** Extract a shared `complete_session()` helper. Scope: medium.

### 7.5 `run_udp_forward_epoch` — 137-line inline `select!` body

**File:** `crates/remote-exec-broker/src/port_forward/udp_bridge.rs:49–187`

The TCP bridge extracted equivalent dispatch into `handle_listen_tunnel_event` and `handle_connect_tunnel_event`. The UDP bridge did not get the same treatment — the listen arm alone is 77 lines and contains a nested match-in-match-in-select.

**Fix:** Extract `handle_udp_listen_tunnel_event` / `handle_udp_connect_tunnel_event`. Establishes structural symmetry with the TCP bridge. Scope: medium.

### 7.6 `PortTunnel::from_stream_with_max_queued_bytes` — 113-line constructor

**File:** `crates/remote-exec-broker/src/port_forward/tunnel.rs:53–166`

Spawns three tasks and wires five channels inline. The reader task body (lines 99–155) is itself 56 lines of inline async block, interleaving heartbeat-ack dispatch, heartbeat-request echo, and data-frame forwarding inside one match in one select.

**Fix:** Extract a named `run_reader_loop`. Scope: small.

---

## Tier 8 — Tests

### 8.1 `assert()` is still used throughout the C++ test suite

**Files:** `tests/test_session_store.cpp`, `tests/test_server_routes_shared.cpp`, `tests/test_server_streaming.cpp` — combined ~130+ `assert()` calls.

Two concrete problems persist:

1. `assert()` is silenced by `NDEBUG`. A release-mode test run passes vacuously.
2. On failure it aborts the process immediately. The first failure hides every subsequent failure in the same binary.

**Fix:** Adopt a lightweight `TEST_ASSERT(cond, msg)` macro (or doctest / Catch2). Scope: large (project-wide test infrastructure).

### 8.2 `test_server_streaming.cpp` remains a 1690-line monolith with no named test functions

**File:** `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`

`grep '^static void test_'` returns nothing. The file is a single TU with helpers and one large test body. The earlier refactor split `test_server_routes_*.cpp` but did not touch this file.

**Fix:** Split into scenario-grouped files (TCP forward, UDP forward, session expiry, worker limits, …). Scope: large.

### 8.3 Duplicated `make_config` between test files

**Files:** `tests/test_server_routes_shared.cpp:33` and `tests/test_server_streaming.cpp:60`

Identical helper, two copies. The shared file already exists.

**Fix:** Move to `test_server_routes_shared.cpp` (or its header), remove duplicate. Scope: trivial.

### 8.4 Tool-name list duplicated in three places

**File:** `crates/remote-exec-broker/src/tools/registry.rs:13–22, 63–81`

`BrokerTool::name()` match arms, `BrokerTool::NAMES`, and the test module's `MCP_TOOL_NAMES` all enumerate every tool. Adding a tool requires three edits.

**Fix:** Drive the contract test from the live `#[tool_router]` registration rather than a hand-maintained third copy. Scope: small.

---

## Tier 9 — Minor / Trivial

### 9.1 `connection_header_has_upgrade` off-by-one bound

**File:** `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp:125` — `while (offset <= value.size())`. The extra iteration is harmless but reads as a bug.

### 9.2 `tunnel_tcp_accept_loop` post-`select!` cancel re-check has no comment

**File:** `crates/remote-exec-host/src/port_forward/tcp.rs:177–200`

The check after `select!` exists to handle a real `accept` vs `cancel` race, but with no comment it will be removed by a future refactor.

### 9.3 `udp_bridge` ignores connect-side `ForwardDrop` frames undocumented

**File:** `crates/remote-exec-broker/src/port_forward/udp_bridge.rs:125–148`

The TCP bridge does the same — likely intentional protocol asymmetry — but neither is documented.

### 9.4 `validated_transport` re-runs at every `DaemonClient::new`

**Files:** `crates/remote-exec-broker/src/daemon_client.rs:169`, `config.rs:330`

Validation runs at config load and again at every client construction. Runtime config error possible from logic already passed at startup.

### 9.5 `write_stdin_inner` re-validates target against session record

**File:** `crates/remote-exec-broker/src/tools/exec.rs:162–167`

A runtime guard duplicating the session store's keying. Not wrong, but inconsistent (no other session operation does this) and undocumented.

### 9.6 `LocalTargetConfig::embedded_port_forward_host_config` builds a throwaway `LocalTargetConfig`

**File:** `crates/remote-exec-broker/src/config.rs:269–284`

If `LocalTargetConfig` gains a required field, this defaulted construction silently uses a value that may be wrong for the port-forward use case.

### 9.7 `send_request_with_policy` takes two generic logging closures

**File:** `crates/remote-exec-broker/src/daemon_client.rs:571–589`

Repeated four times across the file with the same captured context. Replace with a `RpcCallContext` struct.

### 9.8 `TransferHeaderPairs` type alias adds no semantic value

**File:** `crates/remote-exec-proto/src/rpc/transfer.rs:102` — `pub type TransferHeaderPairs = Vec<(&'static str, String)>;` — used twice as a return type; the alias is harder to grep than the concrete type.

### 9.9 RAII handle wrappers duplicated in C++

`UniqueFd` (`process_session_posix.cpp`), `UniqueSocket` (`server_transport.cpp`), `ScopedDirHandle` / `ScopedFindHandle` (`transfer_ops_fs.cpp`) — three implementations of the same pattern.

### 9.10 Pre-C++11 explicit iterator loops mixed with C++11 lambdas in the same files

`session_store.cpp`, `connection_manager.cpp`, `port_tunnel_session.cpp`. Read-only loops can become range-for.

---

## Priority Summary

| Severity | Count | Examples |
|----------|-------|----------|
| Tier 1 (security / data corruption) | 2 | Header injection, patch `.tmp` race |
| Tier 2 (correctness) | 10 | Wire-table drift, warning threshold ignores config, TOCTOU on session insert, generation/tunnel torn state, connect tunnel leak |
| Tier 3 (layering / encapsulation) | 6 | port-forward `rpc_error` wrong semantics, `daemon_client` kitchen sink, `LiveSession` open struct |
| Tier 4 (lock contention) | 5 | O(n) under write lock, `close_lock` across I/O, 3× timeout |
| Tier 5 (type safety) | 2 | C++ overwrite_mode string, `NULL` vs `nullptr` |
| Tier 6 (over-abstraction) | 11 | Eight single-use helpers + `ToolOperationError`, `TcpReadLoopTarget`, C++ overload cascade |
| Tier 7 (function size / hidden state) | 6 | `queue_or_send_tcp_connect_frame`, `forward_exec_write`, C++ `write_stdin`, UDP epoch loop |
| Tier 8 (tests) | 4 | C++ `assert()`, 1690-line streaming test, duplicated helpers, tool-name list |
| Tier 9 (minor) | 10 | Off-by-one read, undocumented races, alias bloat |

### Recommended order of attack

1. **1.1 and 1.2** — security primitives, no churn.
2. **2.1, 2.2** — wire-table drift and broken warning threshold; both small fixes with real production impact.
3. **2.3–2.8** — race/leak cluster; each is small and localized.
4. **3.1, 3.5** — wrong error semantics in port-forward and `LiveSession` encapsulation; modest blast radius for clear correctness/observability gains.
5. **4.1, 4.2** — lock-hold improvements with measurable effect under load.
6. **Tier 6** — sweep the single-use helpers in one focused pass to avoid drip-feed churn.
7. **Tier 8** — test infrastructure decision (assert macro / framework) before splitting `test_server_streaming.cpp`.
