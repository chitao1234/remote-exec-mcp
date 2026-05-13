# Code Quality Audit

> Analysis date: 2026-05-13. Read-only — no code was modified.

---

## 1. Code Duplication

### 1.1 `TcpReadLoopTarget` enum — all methods are pure dispatch duplication

**File:** `crates/remote-exec-host/src/port_forward/tcp.rs:42–124`

Every method on `TcpReadLoopTarget` (`send_frame`, `send_error_code`, `send_read_failed`, `cleanup_stream`, `cancel_stream`, `clear_cancel`) has identical bodies in both `Connect` and `Listen` arms. ~80 lines of boilerplate that a shared `TunnelContext` trait (with `fn tx()`, `fn generation()`, `fn tcp_streams()`) would eliminate entirely.

The same pattern recurs in `tunnel_tcp_data` (lines 454–503) and `tunnel_tcp_eof` (lines 532–574), where `Listen` and `Connect` arms are structurally identical.

### 1.2 `TransferError` and `ImageError` are structurally identical

**File:** `crates/remote-exec-host/src/error.rs`

Both types share the same structure: a `kind` enum + `message` string, named constructors, a `code()` method, an `into_host_rpc_error()` method with identical logic (log, then return bad_request or internal), and `Display`/`Error` impls. A generic `DomainError<K>` or a declarative macro would collapse ~150 lines of duplication.

### 1.3 `new_session` and `new_session_for_test` are copy-pastes

**File:** `crates/remote-exec-host/src/port_forward/tunnel.rs:293–322`

The only difference is the source of the `CancellationToken`. A single constructor on `SessionState` taking a `CancellationToken` parameter would eliminate the test-only copy.

### 1.4 Six identical backend dispatch methods in `TargetHandle`

**File:** `crates/remote-exec-broker/src/target/handle.rs:93–158`

Every method (`target_info`, `exec_start`, `exec_write`, `patch_apply`, `image_read`, `transfer_path_info`) follows the same pattern:

```rust
pub async fn method(&self, req: &Req) -> Result<Resp, DaemonClientError> {
    match &self.backend {
        TargetBackend::Remote(client) => client.method(req).await,
        TargetBackend::Local(client) => client.method(req).await,
    }
}
```

A macro or trait-object dispatch would reduce this to a single definition.

### 1.5 `mark_reconnecting` and `mark_connect_reopening_after_listen_recovery` share ~80% logic

**File:** `crates/remote-exec-broker/src/port_forward/store.rs:135–178`

Both methods ensure capacity, get a mutable entry, increment `reconnect_attempts`, set `last_reconnect_at`, and derive phase. They differ only in which side states they update. A shared private helper would remove the duplication.

### 1.6 Duplicated `validate_existing_directory`

**Files:**
- `crates/remote-exec-broker/src/config.rs:425–434`
- `crates/remote-exec-host/src/config/mod.rs:241–250`

Identical logic. The broker already depends on `remote-exec-host`; making the host version `pub` would eliminate the copy.

### 1.7 Duplicated `normalized_default_workdir` pattern

**Files:**
- `crates/remote-exec-host/src/config/mod.rs:202`
- `crates/remote-exec-daemon/src/config/mod.rs:114`
- `crates/remote-exec-broker/src/config.rs:239`

All three call `remote_exec_host::config::normalize_configured_workdir` with the same arguments. The daemon and broker re-implement what the host already provides.

### 1.8 Duplicated `toml_string` helper in test support

**Files:**
- `crates/remote-exec-broker/tests/support/spawners.rs:84`
- `crates/remote-exec-daemon/tests/support/spawn.rs:26`

Identical function copy-pasted between broker and daemon test support. Could live in `tests/support/` (the shared module already used for `transfer_archive.rs`).

### 1.9 Duplicated `wait_until_ready` polling patterns

Three independent implementations of the same timeout+poll loop:
- `crates/remote-exec-broker/tests/support/spawners.rs` — `wait_until_ready_http` (line 967) and `wait_until_ready_mcp_http` (line 993)
- `crates/remote-exec-daemon/tests/support/spawn.rs` — `wait_until_ready` (line 130)

### 1.10 Duplicated `write_text_file` in C++ tests

**Files:**
- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp:79`
- `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp:72`

Same function, same logic, different file.

### 1.11 ~25 near-duplicate spawner functions

**File:** `crates/remote-exec-broker/tests/support/spawners.rs` (1048 lines)

Functions like `spawn_broker_with_stub_daemon` vs `spawn_broker_with_stub_daemon_and_structured_content_disabled` differ by a single config line. `spawn_broker_with_local_target` vs `spawn_broker_with_local_target_apply_patch_encoding_autodetect` differ by one boolean. A builder pattern would collapse most of these.

### 1.12 Duplicated tracing/error patterns in `daemon_client.rs`

**File:** `crates/remote-exec-broker/src/daemon_client.rs:297–429`

`send_transfer_export_request`, `send_transfer_import_request`, and `decode_transfer_import_response` all follow the same pattern: call, log on error with target/base_url/elapsed_ms/error, map to `DaemonClientError`. A helper taking a closure and context would remove the repetition.

### 1.13 Polling loop pattern duplicated in exec handlers

**Files:**
- `crates/remote-exec-host/src/exec/handlers.rs:40–59`
- `crates/remote-exec-host/src/exec/support.rs:75–97`

Both implement the same poll-sleep-accumulate loop. The `exec_start_local` version differs only in the exit-check callback, which could be a parameter.

### 1.14 `require_*` methods in `active.rs` are repetitive

**File:** `crates/remote-exec-host/src/port_forward/active.rs:111–228`

`require_protocol`, `require_listen_session`, `require_connect_tunnel`, and `require_bind_target` all follow the same pattern: match on enum, check protocol, return formatted error. ~120 lines reducible to ~30 with a macro or generic helper.

---

## 2. Function and Struct Size / Complexity

### 2.1 `build_opened_forward` — 107 lines

**File:** `crates/remote-exec-broker/src/port_forward/supervisor/open.rs:207–314`

Destructures context, opens listen/connect tunnels, sends frames, waits for listener ready, builds session control, creates epoch, identity, runtime, and record — all in one function. Should be decomposed into at least `send_listen_request`, `build_runtime`, and `build_record`.

### 2.2 `tunnel_tcp_accept_loop` — 99 lines

**File:** `crates/remote-exec-host/src/port_forward/tcp.rs:193–291`

Handles accept, permit acquisition, stream splitting, entry insertion, frame construction, error handling, and spawning the read loop. Natural split: `accept_connection`, `register_stream`, `notify_accept`.

### 2.3 `exec_write_local` — 100 lines

**File:** `crates/remote-exec-host/src/exec/handlers.rs:75–174`

Session locking, PTY resize validation, stdin validation, write polling, exit detection, and response building in one function. The session-lock-and-validate phase (lines 87–126) could be a helper.

### 2.4 `DaemonConfig` — 18 fields

**File:** `crates/remote-exec-daemon/src/config/mod.rs:27–62`

A flat struct with 18 fields. Natural groupings: transport config (`transport`, `http_auth`, `tls`), exec config (`pty`, `default_shell`, `yield_time`, `max_open_sessions`, `allow_loginshell`), transfer config (`enable_transfer_compression`, `transfer_limits`).

### 2.5 `HostRuntimeState` — 13 fields

**File:** `crates/remote-exec-host/src/state.rs:41–54`

Port-forward state (`port_forward_sessions`, `port_forward_limiter`) and exec state (`sessions`) could be sub-structs.

### 2.6 `SessionState` — 10 fields, 6 Mutex-wrapped

**File:** `crates/remote-exec-host/src/port_forward/session.rs:21–32`

Retained resources (`retained_listener`, `retained_udp_bind`) and session lifecycle (`resume_deadline`, `expiry_task`) could be grouped into sub-structs.

### 2.7 `RpcErrorCode` — 38 variants in a flat enum

**File:** `crates/remote-exec-proto/src/rpc/error.rs:12–59`

Namespacing (e.g., `RpcErrorCode::Port(PortErrorCode)`, `RpcErrorCode::Transfer(TransferErrorCode)`) would make pattern matching more manageable and group related errors.

### 2.8 Deeply nested control flow in `tunnel_read_loop`

**File:** `crates/remote-exec-host/src/port_forward/tunnel.rs:109–161`

`tokio::select!` containing a `match` on the frame result, which itself has an `if` for `ErrorKind::UnexpectedEof`, then a post-select `if let Err` + `continue`. Four levels of nesting.

### 2.9 `close_tunnel_runtime` acquires the same lock twice

**File:** `crates/remote-exec-host/src/port_forward/tunnel.rs:464–484`

`tunnel.active` is locked for a read check, then locked again for `.take()`. Between the two acquisitions another task could modify the state. Should be a single lock acquisition.

---

## 3. Error Handling Inconsistencies

### 3.1 `expect()` in non-test production code

- `crates/remote-exec-daemon/src/http/request_log.rs:17` — panics if a request ID contains invalid header characters
- `crates/remote-exec-proto/src/wire.rs:9` — panics on a missing wire mapping variant; would be caught at compile time with a `strum` derive

### 3.2 Mixed `anyhow` and custom error types without clear boundaries

Tool functions return `anyhow::Result<T>`; `TargetHandle` methods return `Result<T, DaemonClientError>`. Conversion happens ad-hoc: sometimes `.into()`, sometimes `normalize_transfer_error`, sometimes `into_anyhow_rpc_message`. No consistent boundary.

### 3.3 `WriteStdinToolError` inconsistency

**File:** `crates/remote-exec-broker/src/tools/exec.rs:22,139`

`write_stdin` wraps errors in a `WriteStdinToolError` newtype while `exec_command` returns raw `anyhow::Error`. Two related tools produce differently formatted error messages.

### 3.4 `decode_rpc_error_strict` returns `Result<DaemonClientError, DaemonClientError>`

**File:** `crates/remote-exec-broker/src/daemon_client.rs:608–618`

Error-in-error return type is an unusual pattern that makes control flow harder to follow.

### 3.5 Port-forward errors skip domain-level logging

Port-forward code constructs `HostRpcError` directly via `rpc_error()` rather than going through a domain error type. This bypasses the logging that `TransferError::into_host_rpc_error()` provides, creating inconsistent observability.

---

## 4. Type Safety Issues

### 4.1 Stringly-typed error codes and warning codes

- `crates/remote-exec-broker/src/port_forward/store.rs:15` — `RECONNECT_LIMIT_EXCEEDED` is a `&str` used both as an error message and a comparison target
- `crates/remote-exec-broker/src/tools/exec.rs:17` — `APPLY_PATCH_WARNING_CODE` is a `&str`; warning codes are a fixed set that would benefit from an enum
- `crates/remote-exec-broker/src/daemon_client.rs:50` — `DaemonClientError::Rpc.code` is `Option<String>` even though it maps to `RpcErrorCode`; the struct could store `Option<RpcErrorCode>` directly

### 4.2 Runtime string inspection for error classification

**File:** `crates/remote-exec-broker/src/port_forward/tunnel.rs`

Functions like `is_backpressure_error`, `is_recoverable_pressure_tunnel_error`, `is_retryable_transport_error` inspect error messages/types at runtime. A typed error enum with variants for each category would make classification compile-time safe.

### 4.3 `void*` context in C++ `ConnectionManager`

**File:** `crates/remote-exec-daemon-cpp/src/connection_manager.cpp:67`

```cpp
bool ConnectionManager::try_start(UniqueSocket client, ConnectionWorkerMain worker_main, void* context)
```

`WorkerRecord` stores the context as `void*`. A `std::function` or template approach would restore type safety.

### 4.4 No `ValidatedConfig` wrapper for daemon config

**File:** `crates/remote-exec-daemon/src/config/mod.rs`

The broker has a `ValidatedBrokerConfig` newtype ensuring configs can't be used before validation. The daemon's `DaemonConfig::load` returns a bare `DaemonConfig` — nothing in the type system prevents using an unvalidated config.

### 4.5 `PortForwardFilter.forward_ids` should be `Option<String>`

**File:** `crates/remote-exec-broker/src/port_forward/store.rs:268`

The `forward_ids: Vec<String>` field is only ever populated with 0 or 1 entries in production code.

---

## 5. Magic Numbers and Undocumented Constants

### 5.1 `WARNING_THRESHOLD` tightly coupled to `DEFAULT_SESSION_LIMIT`

**File:** `crates/remote-exec-host/src/exec/store.rs:11–13`

```rust
const DEFAULT_SESSION_LIMIT: usize = 64;
const WARNING_THRESHOLD: usize = 60;
```

If `DEFAULT_SESSION_LIMIT` changes, `WARNING_THRESHOLD` silently becomes wrong. Should be derived: `DEFAULT_SESSION_LIMIT - 4` or `DEFAULT_SESSION_LIMIT * 15 / 16`.

The same issue exists in the C++ daemon: `test_session_store.cpp:837` hardcodes `60` iterations to match `WARNING_THRESHOLD` without referencing the constant.

### 5.2 Undocumented magic numbers

- `crates/remote-exec-host/src/port_forward/tunnel.rs:105` — `100ms` timeout for draining the writer task with no explanation
- `crates/remote-exec-host/src/port_forward/tunnel.rs:56` — `128` frame queue capacity with no explanation
- `crates/remote-exec-host/src/exec/support.rs:160–163` — 3-byte (6 hex char) chunk ID with no documentation on collision probability

---

## 6. C++ Code Quality

### 6.1 Monolithic test functions

**Files:** `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`, `test_server_routes_shared.cpp`

- `assert_stdin_and_tty_behavior` (lines 409–611): ~200 lines testing 5 distinct behaviors
- `assert_pruning_and_recency_behavior` (lines 613–821): ~200 lines testing 4 distinct behaviors
- `run_platform_neutral_server_route_tests` calls ~10 large assertion functions sequentially — a failure in one skips all subsequent assertions with no isolation

### 6.2 Missing RAII for environment variable manipulation

**File:** `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp:243–272`

`PATH` is manually saved and restored with `setenv`/`unsetenv`. If any assertion between save and restore fails, the environment is left corrupted for subsequent tests. An RAII guard class would fix this.

### 6.3 `DIR*` and `HANDLE` not wrapped in RAII

**File:** `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp:168–227`

`closedir(dir)` and `FindClose(handle)` are called manually. If an exception is thrown inside the iteration loop, the handle leaks. RAII wrappers would make this safe.

### 6.4 Silent no-op on double-release in `release_counter`

**File:** `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp:48–55`

If `current` is loaded as 0, the function returns without decrementing and without any diagnostic. A double-release bug would be silently swallowed.

### 6.5 Raw `assert()` instead of a test framework

All C++ tests use raw `assert()`. On failure: no expected-vs-actual message, no test name, process aborts immediately with no cleanup, no way to run a subset. Even a lightweight framework (doctest, Catch2) would significantly improve debuggability.

---

## 7. Dependency Management

### 7.1 `x509-parser` not in workspace dependencies

**File:** `crates/remote-exec-pki/Cargo.toml:15`

```toml
x509-parser = "0.18"
```

Every other shared dependency uses `[workspace.dependencies]`. This crate-local declaration is easy to miss during version bumps.

### 7.2 Potential duplicate `x509-parser` versions

**File:** `crates/remote-exec-pki/Cargo.toml:10,15`

`rcgen` is enabled with `features = ["x509-parser"]`, which already pulls in `x509-parser` as a transitive dependency. The explicit direct dependency creates two paths to the same crate. If `rcgen` upgrades its `x509-parser` version, two incompatible versions could end up in the dependency tree.

### 7.3 Overly broad `tokio` features in workspace

**File:** `Cargo.toml:56`

Nearly every tokio feature is enabled workspace-wide. Crates like `remote-exec-proto` and `remote-exec-pki` likely don't need `process`, `signal`, `net`, or `rt-multi-thread`. This increases compile times and binary sizes for crates that only need a subset.

### 7.4 `tempfile` as a runtime dependency in the broker

**File:** `crates/remote-exec-broker/Cargo.toml:37`

`tempfile` is in `[dependencies]`, not `[dev-dependencies]`. If it is only used in tests, it should be moved.

---

## 8. Security Concerns

### 8.1 CA certificate has unconstrained path length

**File:** `crates/remote-exec-pki/src/generate.rs:116`

```rust
params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
```

Any certificate signed by this CA could itself act as a CA and sign further certificates. `BasicConstraints::Constrained(0)` would prevent leaf certs from being misused as intermediate CAs.

### 8.2 No explicit certificate validity periods

**File:** `crates/remote-exec-pki/src/generate.rs`

Neither `generate_ca` nor `issue_broker_cert`/`issue_daemon_cert` set `not_before`/`not_after`. Certificates use rcgen's default (currently 1 year). For a CA certificate, this is short and will cause silent failures on expiry.

---

## 9. Minor / Low-Priority

### 9.1 `validate_existing_directory` called twice on the same path

**File:** `crates/remote-exec-broker/src/config.rs:304–321`

After `normalize_paths()` sets `local.default_workdir`, `validate()` calls `normalized_default_workdir()` again instead of using the already-normalized field.

### 9.2 Generic `Log` parameter in `ensure_success` always used the same way

**File:** `crates/remote-exec-broker/src/daemon_client.rs:516`

The `Log: FnOnce(StatusCode)` generic is always a `tracing::warn!` closure. Logging internally and removing the generic would simplify the signature.

### 9.3 No-op callback in `handle_forward_loop_control` for TCP

**File:** `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:46`

The `before_connect_recover` callback is `|| async {}` on the TCP path. The TCP bridge pays the generic complexity cost for a feature only the UDP bridge uses.

### 9.4 `SessionStore::lock_if_current_after_guard` — triple lock acquisition

**File:** `crates/remote-exec-host/src/exec/store.rs:143–166`

Three separate lock acquisitions (`lock_owned`, `read`, `write` via `touch_if_current`) to perform a single guarded update. The pattern is correct but a comment explaining why the double-check is necessary would help maintainability.

### 9.5 Repetitive `ExecStartRequest` construction in daemon tests

**File:** `crates/remote-exec-daemon/tests/exec_rpc/unix.rs`

Nearly every test constructs `ExecStartRequest` with the same boilerplate fields. A builder or helper like `exec_start_request("cmd").pty(true).yield_time(250)` would reduce noise.

---

## Priority Summary

| Priority | Issue | Location |
|----------|-------|----------|
| High | 6 identical backend dispatch methods | `target/handle.rs:93–158` |
| High | `TcpReadLoopTarget` — all methods are pure duplication | `tcp.rs:42–124` |
| High | `TransferError` / `ImageError` structurally identical | `error.rs` |
| High | ~25 near-duplicate spawner functions | `spawners.rs` |
| High | Monolithic 200-line C++ test functions | `test_session_store.cpp` |
| High | CA with unconstrained path length | `generate.rs:116` |
| High | No explicit certificate validity periods | `generate.rs` |
| Medium | `build_opened_forward` — 107 lines | `supervisor/open.rs:207–314` |
| Medium | `DaemonConfig` — 18 flat fields | `daemon/config/mod.rs:27–62` |
| Medium | `expect()` in non-test production code | `request_log.rs:17`, `wire.rs:9` |
| Medium | Stringly-typed error/warning codes | Multiple |
| Medium | Missing RAII for env vars and file handles in C++ | `test_session_store.cpp`, `transfer_ops_fs.cpp` |
| Medium | `void*` context in `ConnectionManager` | `connection_manager.cpp:67` |
| Medium | No `ValidatedConfig` wrapper for daemon | `daemon/config/mod.rs` |
| Medium | `x509-parser` not in workspace deps + potential duplicate | `remote-exec-pki/Cargo.toml` |
| Medium | Duplicated `validate_existing_directory` | `broker/config.rs`, `host/config/mod.rs` |
| Low | `WARNING_THRESHOLD` not derived from `DEFAULT_SESSION_LIMIT` | `store.rs:11–13` |
| Low | Undocumented magic numbers (100ms, 128, 3-byte ID) | Multiple |
| Low | Raw `assert()` in all C++ tests | C++ test files |
| Low | Overly broad tokio features workspace-wide | `Cargo.toml:56` |
| Low | `tempfile` in runtime deps (should be dev-dep) | `broker/Cargo.toml:37` |
