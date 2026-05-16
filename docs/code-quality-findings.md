# Code Quality Findings

Analysis of code smells, structural issues, and ad hoc patterns across the
`remote-exec-mcp` workspace. All file paths and line numbers have been verified
against the current codebase.

---

## 1. Duplicated Logic

These are the most impactful issues overall. Each instance creates
synchronization burden and divergence risk.

### 1.1 Identical `lexical_normalize` / `normalize_path` functions

**Files:**
- `crates/remote-exec-host/src/sandbox.rs:204-218` (`lexical_normalize`)
- `crates/remote-exec-host/src/image.rs:78-90` (`normalize_path`)

Both implement the same component-walking path normalization. The only cosmetic
difference is `let _ = normalized.pop()` vs `normalized.pop()`.

**Fix:** Extract into a shared utility in `remote-exec-host` (e.g.,
`host_path::lexical_normalize`) and call it from both modules.

### 1.2 `HostRuntimeConfig` and `EmbeddedHostConfig` are identical structs

**File:** `crates/remote-exec-host/src/config/mod.rs:35-68`

Both structs have the same 14 fields. `into_host_runtime_config` (line 181) is a
field-by-field copy. Additionally, `DaemonConfig` in
`crates/remote-exec-daemon/src/config/mod.rs:119-157` has two methods
(`embedded_host_config` and `into_embedded_host_config`) that duplicate the same
14-field mapping with clone-vs-move as the only difference.

**Fix:** Eliminate `EmbeddedHostConfig` entirely and use `HostRuntimeConfig`
directly. Or make `EmbeddedHostConfig` a newtype wrapper. The daemon's two
conversion methods collapse to one when the consuming type is by-value.

### 1.3 `RpcCallContext` logging -- five methods with identical match arms

**File:** `crates/remote-exec-broker/src/daemon_client/mod.rs:205-316`

`log_completed`, `log_transport_error`, `log_status_error`, `log_read_error`,
and `log_decode_error` each contain the same `match self.subject { Path(..) =>
..., DestinationPath(..) => ... }` dispatch. The shared fields
(`target_name`, `base_url`, `elapsed_ms`) are repeated in every arm of every
method.

**Fix:** Extract the common fields into a `tracing::Span` at construction time.
Each log method then adds its specific fields without matching on subject.

### 1.4 Wire-value mapping duplication (const array + manual match)

**File:** `crates/remote-exec-proto/src/transfer.rs:51-156`

Four types (`TransferSourceType`, `TransferOverwrite`, `TransferSymlinkMode`,
`TransferCompression`) each define both a `const` wire-value array AND a manual
`match` in `wire_value()` with the same string literals repeated. The same
pattern appears in `crates/remote-exec-proto/src/rpc/warning.rs:11-38` for
`WarningCode`. Meanwhile, `RpcErrorCode` in `rpc/error.rs` already solves this
cleanly with a declarative macro.

**Fix:** Use the `rpc_error_code_mappings!` macro approach (or a simpler lookup
over the const array) for all wire-value types. A single source of truth
eliminates the risk of one side diverging.

### 1.5 `TargetBackend` dispatch boilerplate

**File:** `crates/remote-exec-broker/src/target/dispatch.rs:12-86`

Five methods (`target_info`, `exec_start`, `exec_write`, `patch_apply`,
`image_read`) have the identical structure: `match self { Remote(c) =>
c.method(req).await, Local(c) => c.method(req).await }`. Both arms call the
same method name with the same signature.

**Fix:** Define a trait (e.g., `DaemonRpcClient`) that both `DaemonClient` and
`LocalDaemonClient` implement, and use `dyn` dispatch or an enum-dispatch macro.

### 1.6 `UdpReadLoopTarget` enum repeats identical match arms

**File:** `crates/remote-exec-host/src/port_forward/udp.rs:42-128`

Four methods (`send_error_code`, `send_read_failed`, `send_forward_drop`,
`close_on_terminal_send_failure`) each match on `Connect`/`Listen` with
identical bodies in both arms. The TCP equivalent in `tcp.rs:58-90` already uses
a `TcpReadLoopContext` trait to abstract over the two context types.

**Fix:** Apply the same trait-based approach that TCP already uses.

### 1.7 C++ duplicated `wait_for_generation_change_locked`

**Files:**
- `crates/remote-exec-daemon-cpp/src/session_store.cpp:112-129`
- `crates/remote-exec-daemon-cpp/src/session_pump.cpp:35-52`

Character-for-character identical function in two anonymous namespaces.

**Fix:** Move to a shared internal header or a common compilation unit.

### 1.8 C++ triplicated "mark resource closed" pattern

**File:** `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`

`mark_tcp_stream_closed` (320-339), `mark_udp_socket_closed` (341-356), and
`mark_retained_listener_closed` (394-409) all follow lock -> check closed flag
-> set closed -> shutdown -> release budget -> notify. Two more variants exist
in `port_tunnel_session.cpp:14-51`.

**Fix:** Template or helper function parameterized on the resource type.

### 1.9 Test helper duplication across crates

| Helper | Broker location | Daemon location |
|--------|----------------|-----------------|
| `TestCerts` / `write_test_certs` | `tests/support/certs.rs:4-37` | `tests/support/certs.rs:3-29` |
| `utf16le_bom_bytes` | `tests/mcp_assets.rs:8-11` | `tests/patch_rpc.rs:6-9` |
| `msys_style_path` | `tests/mcp_transfer.rs:13-28` | `tests/support/mod.rs:27-53` |
| `assert_pem_pair` | — | PKI: `tests/ca_reuse.rs:6-9` and `tests/dev_init_bundle.rs:15-18` |
| `admin()` / `assert_success()` | — | Admin: `tests/certs_issue.rs:3-12` and `tests/dev_init.rs:4-14` |

**Fix:** Extract shared helpers into `remote-exec-test-support` (which already
exists) behind feature flags, or into per-crate `tests/support/` modules.

---

## 2. Stringly-Typed Patterns

Pervasive use of bare `String` where newtypes would prevent misuse.

### 2.1 Identifiers are all bare `String`

Forward IDs, session IDs, daemon instance IDs, and endpoint addresses are all
`String` throughout the broker (`port_forward/store.rs`, `supervisor.rs`) and
host (`port_forward/session.rs:22`) crates. A `forward_id` can be accidentally
passed where a `session_id` is expected with no compiler error.

**Fix:** Introduce newtypes (e.g., `struct ForwardId(String)`,
`struct SessionId(String)`) for the most-used identifiers. Start with
`ForwardId` and `DaemonSessionId` which appear most frequently.

### 2.2 `RpcErrorBody.code` is `String` instead of `RpcErrorCode`

**File:** `crates/remote-exec-proto/src/rpc/error.rs:4-7`

The `code` field is a freeform `String`. Constructors do the right thing via
`wire_value().to_string()`, but the `pub` field allows any arbitrary string.
Same issue applies to `ExecWarning.code` and `TransferWarning.code`.

**Fix:** Use `RpcErrorCode` / `WarningCode` as the field type and serialize
via serde `rename` or a custom serializer. Or make the fields private with
validated constructors.

### 2.3 Tunnel metadata uses `String` for enumerated reasons/codes

**File:** `crates/remote-exec-proto/src/port_tunnel/meta.rs`

`TunnelCloseMeta.reason` (line 53), `TunnelErrorMeta.code` (line 94),
`ForwardDropMeta.reason` (line 86), and `ForwardRecoveringMeta.reason` (line 66)
are all `String` despite having defined const values.

**Fix:** Replace with `enum` types for the known values, with a `String`
fallback variant for forward compatibility.

### 2.4 `Timestamp(pub String)` with no validation

**File:** `crates/remote-exec-proto/src/public/forward_ports.rs:108-110`

A transparent newtype over `String` that does not enforce any timestamp format.

### 2.5 `HealthCheckResponse.status` is `String`

**File:** `crates/remote-exec-proto/src/rpc/target.rs:6-11`

Always set to `"ok"` but typed as freeform `String`.

---

## 3. Parallel Type Hierarchies

### 3.1 `ForwardPortProtocol` vs `TunnelForwardProtocol`

Both are `{ Tcp, Udp }` enums with trivial `From` conversions. Same for
`ForwardPortSideRole` vs `TunnelRole` (both `{ Listen, Connect }`).

**Fix:** If these must remain separate for API boundary reasons, consider a
macro to generate both from one definition. Otherwise unify.

### 3.2 `ExecWarning` and `TransferWarning` are identical structs

**Files:**
- `crates/remote-exec-proto/src/rpc/exec.rs:165-169`
- `crates/remote-exec-proto/src/rpc/transfer/types.rs:7-11`

Both are `{ code: String, message: String }`.

**Fix:** A shared `Warning` type.

### 3.3 Three names for one type: `TransferImportSpec` / `Request` / `Metadata`

**File:** `crates/remote-exec-proto/src/transfer.rs:187-194`

Two type aliases and a `.metadata()` method that is just `.clone()`.

**Fix:** Keep one canonical name and remove the aliases unless they serve a
documented API-boundary purpose.

---

## 4. Excessive Parameters

Functions with 5+ parameters, many of which simply relay fields from one struct
to another.

| Function | File | Params | Notes |
|----------|------|--------|-------|
| `build_exec_command_input` | `broker/src/cli/input.rs:11` | 8 | Pointless intermediary -- caller unpacks a struct to call this |
| `build_write_stdin_input` | `broker/src/cli/input.rs:33` | 8 | Same issue |
| `build_transfer_files_input` | `broker/src/cli/input.rs:92` | 7 | Same issue |
| `open_tunnel_with_role` | `broker/src/port_forward/supervisor/tunnel_open.rs:121` | 8 | Has `#[allow(clippy::too_many_arguments)]` |
| `ForwardPortEntry::new_open` | `proto/src/public/forward_ports.rs:171` | 7 | All `String` params, easy to transpose |
| `send_tunnel_error` | `host/src/port_forward/active.rs:319` | 6 | |
| `serve_streamable_http` | `broker/src/mcp_server.rs:323` | 6 | Could take config struct |
| `SessionStore::start_command` (C++) | `daemon-cpp/src/session_store.cpp:419` | 11 | Needs a request struct |

**Fix:** For the CLI `build_*_input` functions, construct the input struct
directly at the call site. For tunnel/forward functions, group related
parameters into context structs (e.g., `ForwardIdentity` for `forward_id` +
`protocol` + `generation`).

---

## 5. Complexity and Nesting

### 5.1 `PortTunnel::from_stream_with_max_queued_bytes` -- 122 lines, 6-level nesting

**File:** `crates/remote-exec-broker/src/port_forward/tunnel.rs:53-175`

The reader task (lines 99-164) is a 65-line nested closure with
`spawn > loop > select! > match > match > try_send` nesting. The writer task
and heartbeat task are also defined inline.

**Fix:** Extract the reader, writer, and heartbeat tasks into named async
functions, consistent with how `tunnel_tcp_write_loop` is already extracted in
`host/src/port_forward/tcp.rs`.

### 5.2 `serve_http1_connections` -- 91 lines, 5-level nesting

**File:** `crates/remote-exec-daemon/src/http_serve.rs:26-117`

The inner connection handler (lines 61-101) is a 40-line async closure inside a
`loop > select! > spawn > match > select!` nest.

**Fix:** Extract the inner connection handler into a named async function.

### 5.3 `exec_command` mixes 6 concerns

**File:** `crates/remote-exec-broker/src/tools/exec.rs:24-98`

Logging, target resolution, identity verification, `apply_patch` interception
detection, the exec RPC call, session registration, and output formatting are
all in one function.

**Fix:** Extract interception check and output formatting into helper functions.

### 5.4 C++ `parse_patch` -- 97 lines

**File:** `crates/remote-exec-daemon-cpp/src/patch_engine.cpp:359-455`

Handles three action types (Add, Delete, Update) with sub-parsing inside a
single `while` loop.

**Fix:** Extract per-action-type parsing functions.

### 5.5 C++ `import_directory_from_tar` -- 91 lines, 4-level nesting

**File:** `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp:406-496`

Dispatches on six tar entry types in a single loop body.

**Fix:** Extract per-entry-type handlers.

---

## 6. Telescoping Function Explosion (Daemon Crate)

**Files:** `crates/remote-exec-daemon/src/lib.rs`, `server.rs`, `tls.rs`,
`tls_enabled.rs`, `tls_disabled.rs`

Approximately 17 serve/run functions across 5 files for fundamentally one
operation. Each "no-shutdown" variant is a one-liner that passes
`std::future::pending::<()>()`. Each "no-listener" variant calls
`bind_listener()` and delegates.

Additionally, `install_crypto_provider()` is called redundantly in both
`run_until` (line 47) and `run_until_on_bound_listener` (line 72) -- safe due
to `OnceLock` but indicates confused ownership.

**Fix:** Eliminate the convenience wrappers. Use a builder or make
`shutdown: impl Future` default to `pending()` at the caller. The
`bind_listener` call can live at the top of the most general variant.

---

## 7. Missing Type Aliases

### 7.1 `(StatusCode, Json<RpcErrorBody>)` repeated 16 times

**Files:** `daemon/src/exec/mod.rs`, `transfer/mod.rs`, `transfer/codec.rs`,
`port_forward.rs`, `image.rs`, `rpc_error.rs`, `patch/mod.rs`

Every daemon RPC handler returns `Result<Json<T>, (StatusCode,
Json<RpcErrorBody>)>` with no centralized alias.

**Fix:** `type RpcError = (StatusCode, Json<RpcErrorBody>);` in a common
module, or an `impl IntoResponse` newtype.

---

## 8. Production Safety Concerns

### 8.1 `.expect()` calls in async production code

| Location | Message |
|----------|---------|
| `broker/src/port_forward/tcp_bridge.rs:830` | `"fully drained tcp stream exists"` |
| `broker/src/port_forward/tcp_bridge.rs:901` | `"pending tcp stream exists"` |
| `daemon/src/rpc_error.rs:13` | `"normalized HostRpcError status is valid"` |

These rely on logical invariants. If violated, the entire tokio task (or daemon)
panics with no recovery.

**Fix:** Return errors instead of panicking. The invariant comments can become
`debug_assert!` checks.

### 8.2 Silent `_ => Ok(None)` catch-alls on frame type matches

**Files:**
- `broker/src/port_forward/tcp_bridge.rs:137,176`
- `broker/src/port_forward/udp_bridge.rs:132,232`

Unrecognized frame types are silently dropped with no logging.

**Fix:** Add `tracing::debug!` for unknown frame types to surface protocol
mismatches during development.

### 8.3 C++ `strerror(errno)` is not thread-safe

**File:** `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`

19 occurrences of `std::strerror(errno)` in a multi-threaded daemon. POSIX does
not guarantee thread safety for `strerror`.

**Fix:** Use `strerror_r` (GNU/POSIX extension).

### 8.4 C++ `ptsname` returns a static buffer

**File:** `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp:160`

`ptsname()` returns a pointer to a static buffer that is not thread-safe.

**Fix:** Use `ptsname_r` on Linux.

### 8.5 C++ recursive `remove_existing_path` with no depth limit

**File:** `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp:94-117`

No maximum recursion depth guard. A deeply nested directory or symlink loop
could cause stack overflow.

**Fix:** Add a depth counter and fail above a reasonable limit (e.g., 256).

---

## 9. C++ Build and Idiom Issues

### 9.1 `-O0` in production builds

**File:** `crates/remote-exec-daemon-cpp/mk/common.mk:5`

```
PROD_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
```

The production binary compiles with zero optimization. This is likely a
development-time setting that was never updated.

**Fix:** Use `-O2` for production builds. Consider separate debug/release
profiles in the Makefile.

### 9.2 Dead `#ifdef` branch

**File:** `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp:104-108`

```cpp
#ifdef _WIN32
    if (!path_utils::remove_directory(path)) {
#else
    if (!path_utils::remove_directory(path)) {
#endif
```

Both branches are character-for-character identical.

**Fix:** Remove the `#ifdef`.

### 9.3 `<windows.h>` before `<winsock2.h>` in `basic_mutex.h`

**File:** `crates/remote-exec-daemon-cpp/include/basic_mutex.h:4-5`

On MSVC, `<winsock2.h>` must be included before `<windows.h>` to avoid
redefinition warnings.

**Fix:** Swap the include order.

### 9.4 `static const` in headers creates per-TU copies

**File:** `crates/remote-exec-daemon-cpp/include/config.h:9-29`

21 `static const` integral variables at file scope. Each translation unit gets
its own copy.

**Fix:** Use `constexpr` (which implies internal linkage for integral types but
is more idiomatic in C++11) or `extern const` with definitions in `config.cpp`.

### 9.5 No `std::make_shared` usage

Every `shared_ptr` is constructed with bare `new` throughout the C++ daemon.
`std::make_shared` is available in C++11 and provides allocation efficiency and
exception safety.

### 9.6 C-style iterator loops instead of range-based `for`

Dozens of instances of `for (std::map<...>::iterator it = map.begin(); it !=
map.end(); ++it)` where range-based `for` would be cleaner and less
error-prone.

### 9.7 Unscoped enums in `patch_engine.cpp`

**File:** `crates/remote-exec-daemon-cpp/src/patch_engine.cpp:27-57`

`PatchKind`, `LineEndingKind`, and `NormalizedPathKind` are unscoped enums.
C++11 `enum class` would provide type safety.

---

## 10. Redundant Public API Surface

### 10.1 Free functions in `path.rs` that just delegate to methods

**File:** `crates/remote-exec-proto/src/path.rs:168-206`

Five public functions (`is_absolute_for_policy`, `normalize_for_system`,
`syntax_eq_for_policy`, `basename_for_policy`, `join_for_policy`) that each
call the corresponding `PathPolicy` method. External callers already use the
methods directly.

**Fix:** Remove the free functions or make them `pub(crate)` if only tests use
them.

### 10.2 `port_tunnel` re-exports wire-level internals

**File:** `crates/remote-exec-proto/src/port_tunnel/mod.rs:4-14`

Re-exports 20+ items including `HEADER_LEN`, `MAX_DATA_LEN`, `PREFACE`,
`encode_frame_meta`, etc. These are wire-level implementation details.

**Fix:** Narrow the public re-exports to the types and functions that other
crates actually need.

---

## 11. Test Quality Issues

### 11.1 Magic strings with 100+ occurrences

`"builder-a"` appears in 111+ test call sites across broker and daemon tests
with no named constant. `"builder-xp"`, `"shared-secret"`, and yield-time
values `250` / `5_000` are also repeated without constants.

**Fix:** Define constants (e.g., `const DEFAULT_TEST_TARGET: &str =
"builder-a"`, `const LIVE_SESSION_YIELD_MS: u64 = 250`).

### 11.2 Excessive `.unwrap()` without context

`transfer_rpc.rs` has 146 `.unwrap()` calls in 1348 lines (~1 per 9 lines),
most without `.expect("context")`. Contrast with `exec_rpc/unix.rs` which has
only 8 in 775 lines.

**Fix:** Convert the most diagnostic-critical unwraps to `.expect()` with path
or operation context.

### 11.3 Multi-concern test functions

`remote_exec_cli_forward_ports_opens_lists_and_closes_local_tcp_forward` in
`broker/tests/mcp_cli.rs:305-410` (106 lines) tests open, connect, list, and
close as a single function. A failure in any step masks the rest.

**Fix:** Split into focused tests that share a fixture.

---

## Summary: Top 10 Highest-Impact Fixes

| # | Issue | Impact | Effort |
|---|-------|--------|--------|
| 1 | Identical `HostRuntimeConfig`/`EmbeddedHostConfig` + daemon converters (1.2) | High -- 3 layers of field duplication | Low |
| 2 | Wire-value mapping const+match duplication across 5 types (1.4) | High -- divergence risk in serialization | Low |
| 3 | `TargetBackend` dispatch boilerplate (1.5) | Medium -- 5 identical methods | Low |
| 4 | `RpcCallContext` 5-method logging duplication (1.3) | Medium -- 110 lines of repetition | Low |
| 5 | Stringly-typed `RpcErrorBody.code` and warning codes (2.2) | Medium -- silent validation gaps | Medium |
| 6 | C++ `-O0` in production builds (9.1) | High -- direct performance impact | Trivial |
| 7 | `PortTunnel::from_stream` 6-level nesting (5.1) | Medium -- readability, testability | Medium |
| 8 | Missing `(StatusCode, Json<RpcErrorBody>)` type alias (7.1) | Low -- 16 repetitions | Trivial |
| 9 | Test helper duplication across crates (1.9) | Medium -- divergence risk | Low |
| 10 | `.expect()` in production async code (8.1) | Medium -- panic risk | Low |
