# Code-Quality Audit — 2026-05-16

Scope: Rust workspace and the C++11 daemon under `crates/`. Findings come from
reading the actual source; subagent claims that did not survive verification
(e.g., `unwrap()`/`expect()` flagged in code that was actually inside
`#[cfg(test)]` modules) have been dropped. Severity is the author's judgement
of risk to correctness, maintainability, or contract drift; it is not a
prescription that anything must change.

The audit is organised by theme rather than by crate, so a single root cause
that crosses crates is not split across sections.

---

## 1. Cross-language contract duplication (HIGH)

The deepest structural issue. The Rust `remote-exec-proto` crate is intended to
be the canonical source of truth for the broker–daemon and public MCP
contracts, but the C++ daemon re-implements several of those contracts
independently with no shared specification. Any divergence is a silent
protocol bug.

### 1.1 Port-tunnel frame codec duplicated

`crates/remote-exec-proto/src/port_tunnel.rs` defines the `Frame` and
`FrameType` enums and their wire numbers. `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`
and `include/port_tunnel_frame.h` re-declare the same enum with identical
numeric values and re-implement encode/decode. Adding a frame type or changing
a field requires synchronised edits in two languages with no compile-time
check.

`crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp:9` —
`PORT_TUNNEL_HEADER_LEN = 16U` is a constant whose value must match the Rust
`HEADER_LEN`.

### 1.2 Path policy / normalization duplicated

`crates/remote-exec-proto/src/path.rs:1-32` defines `PathPolicy`, `PathStyle`,
`linux_path_policy`, `windows_path_policy`, and `host_policy`.
`crates/remote-exec-daemon-cpp/src/path_policy.cpp` and `path_compare.cpp`
re-implement the same surface independently. There is no shared corpus of
edge-case tests (UNC, drive letters, trailing separators, cygdrive) executed
against both implementations.

### 1.3 Sandbox enforcement duplicated

`crates/remote-exec-proto/src/sandbox.rs` plus
`crates/remote-exec-host/src/sandbox.rs` own the Rust enforcement.
`crates/remote-exec-daemon-cpp/src/filesystem_sandbox.cpp` and
`include/filesystem_sandbox.h` re-implement the rule compilation and access
check. A rule change must be applied in two places.

### 1.4 Transfer archive logic duplicated

`crates/remote-exec-host/src/transfer/archive/` and the C++
`transfer_ops_import.cpp` / `transfer_ops_export.cpp` / `transfer_ops_tar.cpp`
implement the same tar-based archive flow with overwrite modes, symlink
handling, and path-traversal rejection. AGENTS.md acknowledges these are
intentionally separate, but the contract surface (overwrite semantics, warning
codes, header values) is duplicated, not specified once.

### How to address

- Treat `remote-exec-proto` as a contract spec: fix wire numbers, header
  names, error codes, and frame layouts in one Rust file plus a sibling
  fixtures file (e.g. golden frames as binary blobs, golden paths as JSON).
- Drive C++ tests from the same fixtures so divergence fails the C++ build
  rather than appearing on the wire. The C++ code stays separate; the
  fixtures are the shared truth.
- For sandbox / overwrite semantics, prefer fixture-driven conformance tests
  (rule-set + path → expected decision) over mirrored implementations.

---

## 2. Oversized files mixing responsibilities (HIGH / MEDIUM)

Several files have grown past the point where one cohesive responsibility is
visible. Splits should follow the seams already present in the code.

### 2.1 broker `port_forward/tcp_bridge.rs` — 1848 lines

Three responsibilities: the epoch event loop, per-stream state-machine
handlers, and a 957-line inline `#[cfg(test)] mod tests` (lines 891–1848).
Tests dominate the file. Move tests to `tests/port_forward_tcp_bridge.rs` (or
a sibling `tcp_bridge/tests.rs`) and split the per-stream handlers
(`handle_listen_tcp_accept`, `handle_connect_tcp_data`, …) into a
`tcp_bridge/streams.rs` submodule.

### 2.2 broker `daemon_client.rs` — 1119 lines

Holds the typed error type, HTTP transport helpers, the `DaemonClient` API,
and four 100-line transfer methods (`transfer_export_to_file`,
`transfer_export_stream`, `transfer_import_from_file`, …). The transfer block
is a separate concern; it should live in `daemon_client/transfer.rs` so the
transport scaffolding is readable in isolation.

### 2.3 broker `config.rs` — 944 lines

Mixes `BrokerConfig` deserialization, target validation, `LocalTargetConfig`
factories, MCP server config, and ~500 lines of tests. Split into
`config/mod.rs` (top-level), `config/target.rs`, `config/local.rs`,
`config/mcp_server.rs`, and `config/tests.rs`.

### 2.4 broker `bin/remote_exec.rs` — 831 lines

A single binary file with CLI definitions, per-tool input builders, endpoint
parsers, and emitters. The per-tool builders (`exec_command_input`,
`write_stdin_input`, …) and the endpoint parser belong in a `cli/` submodule
under `src/`.

### 2.5 host `port_forward/port_tunnel_tests.rs` — 1390 lines under `src/`

`crates/remote-exec-host/src/port_forward/mod.rs:92-93` includes it via
`#[cfg(test)] mod port_tunnel_tests;`. The `cfg(test)` gate is correct, so
this is not a production-bloat bug, but a 1390-line test file living next to
production code is a navigation hazard. Move to `crates/remote-exec-host/tests/port_tunnel_tests.rs`,
or split into focused integration files (`tests/port_tunnel_handshake.rs`,
`tests/port_tunnel_close.rs`, etc.).

### 2.6 daemon `config/tests.rs` — 622 lines

`crates/remote-exec-daemon/src/config/tests.rs` is larger than the module it
tests (254 lines). The tests are repetitive: each one creates a tempdir,
writes a TOML, calls `DaemonConfig::load`, asserts one field. A
table-driven helper (`Vec<(toml_str, predicate)>`) would shrink this by
60–70%.

### 2.7 proto `rpc/transfer.rs` — 606 lines, `public.rs` — 414 lines

`rpc/transfer.rs` mixes header-name constants, the `TransferHeaders` struct,
warning factories, the `TransferHeaderError` typed error, eight free
encode/decode functions, `TransferImportResponse`, and 250 lines of tests.
`public.rs` puts five distinct feature areas (exec, transfer, patch, image,
forward_ports) in one flat file; the forward-ports section alone is 178
lines. Both should split by feature area.

### 2.8 C++ daemon TUs

`process_session_posix.cpp` (630), `patch_engine.cpp` (591),
`session_store.cpp` (560), `transfer_ops_import.cpp` (553),
`server_transport.cpp` (514), `port_tunnel_session.cpp` (454),
`port_tunnel.cpp` (418). Each conflates several responsibilities; the
clearest split candidate is `server_transport.cpp` (see §4.2).

---

## 3. Schema and contract smells in `remote-exec-proto` (MEDIUM)

### 3.2 `ForwardPortsAction` mirrors `ForwardPortsInput`

`public.rs:225-231` defines `ForwardPortsAction { Open, List, Close }` whose
sole purpose is to echo the input variant back into
`ForwardPortsResult.action`. The result discriminant is fully derivable from
the request. Drop the field, or drop the enum and use `&'static str` if the
echo is needed for client-side routing.

### 3.3 `ForwardPortProtocol` vs `TunnelForwardProtocol`

Two enums with identical variants for the same concept across
`public.rs:213` and the port-tunnel meta module. Any new protocol variant
must be added in two places. Either re-export one as the other, or generate
both from a single source.

### 3.4 Free `*_for_policy` functions instead of methods

`crates/remote-exec-proto/src/path.rs:103-186` exposes
`is_absolute_for_policy(policy, raw)`, `normalize_for_system(policy, raw)`,
`join_for_policy(policy, base, child)`. This is a C-style API where
`PathPolicy` is just a tag. Methods on `PathPolicy` (or a small `PathOps`
trait) would read better at call sites and make discovery via type-driven
search easier.

### 3.5 `EmptyResponse {}` zero-field struct

`crates/remote-exec-proto/src/rpc/image.rs:19` defines
`pub struct EmptyResponse {}` and re-exports it. If the endpoint returns
nothing, use `()`; if it returns something, name what it returns. The
current name documents the absence of content rather than the response.

### 3.6 `wire.rs` is nine lines

`crates/remote-exec-proto/src/wire.rs` is a single 9-line helper. It does
not justify a separate module; inline it where used or fold into a `util`
module.

### 3.7 `sandbox/` subdirectory holds 22 lines

`crates/remote-exec-proto/src/sandbox.rs` is a 3-line re-export pointing at
`sandbox/types.rs` (19 lines). One file is enough.

---

## 4. Cross-platform conditional-compilation density (MEDIUM)

### 4.1 `basic_mutex.cpp` — every method body split by `#ifdef`

`crates/remote-exec-daemon-cpp/src/basic_mutex.cpp` reimplements
`std::mutex` / `std::condition_variable` for C++11. Of 122 lines, 32 are
preprocessor directives — every method is interleaved POSIX/Win32. The
canonical platform-abstraction pattern is `basic_mutex_posix.cpp` plus
`basic_mutex_win32.cpp` behind a single header.

A latent concern is the Win32 broadcast/signal path:
`basic_mutex.cpp:97,107` use `InterlockedCompareExchange(&waiters_, 0, 0)`
to peek the waiter count before `SetEvent`. This is not a TOCTOU per se,
but it is an unconventional emulation of `condition_variable` and worth
isolating in its own file with a comment explaining the protocol.

### 4.2 `server_transport.cpp` — 42 `#if/#else/#endif` directives across 514 lines

Roughly one preprocessor branch every 12 lines. Platform-specific socket
init, `SOCKET` aliasing, `cloexec`, and error-message formatting are inline
rather than isolated. Extracting `socket_posix.cpp` / `socket_win32.cpp`
behind a 5-method `Socket` interface would make the surface readable.

### 4.3 `crates/remote-exec-host/src/exec/shell.rs` no fallback `cfg`

`exec/shell.rs:17,26` defines `resolve_default_shell` twice — once for
`#[cfg(unix)]`, once for `#[cfg(windows)]`. There is no
`#[cfg(not(any(unix, windows)))]` arm. On any other target the build fails
silently in a confusing way; a `compile_error!` with a clear message would
be kinder.

---

## 5. Test code under `src/` and inline test bloat (MEDIUM / LOW)

All cases are properly `#[cfg(test)]`-gated, so they don't bloat the
production binary. The cost is navigability and the temptation to expose
production symbols as `pub(super)` only because the test file lives next
door.

| Location | Lines | Notes |
|---|---|---|
| `remote-exec-host/src/port_forward/port_tunnel_tests.rs` | 1390 | gated at `mod.rs:92`; consider `tests/` |
| `remote-exec-broker/src/port_forward/tcp_bridge.rs` (lines 891–1848) | 957 | inline tests dominate the file |
| `remote-exec-broker/src/config.rs` | ~500 inline | bundle into `config/tests.rs` after split |
| `remote-exec-daemon/src/config/tests.rs` | 622 | repetitive; table-driven would shrink it |
| `remote-exec-host/src/exec/store/tests.rs` | 396 | included via `store.rs:327` |
| `remote-exec-host/src/exec/shell/windows.rs` | ~180 of 492 | inline tests; promote to `windows/tests.rs` |

---

## 6. Local duplication and missed abstractions (MEDIUM)

### 6.1 `host_path_policy()` duplicates `host_policy()`

`crates/remote-exec-host/src/host_path.rs:8-18` defines
`host_path_policy()` whose body is structurally identical to
`crates/remote-exec-proto/src/path.rs:26-32`'s `host_policy()`. The host
crate already imports `windows_path_policy` and `linux_path_policy` from
proto; it should re-export `host_policy` directly instead of re-wrapping.

### 6.2 `path_is_within` wrapper flips arguments

`crates/remote-exec-host/src/sandbox.rs:221-223`:

```rust
fn path_is_within(root: &Path, path: &Path) -> bool {
    path_compare::path_is_within(path, root)
}
```

`path_compare::path_is_within(path, root)` — the canonical version — takes
`(path, root)`. The local wrapper flips the order. Two callers in the same
crate now have to remember which argument order they are working with. The
wrapper should be deleted and the call sites updated to use the canonical
order.

### 6.3 `embedded_host_config` vs `embedded_port_forward_host_config`

`crates/remote-exec-broker/src/config.rs:253-296` defines two
`EmbeddedHostConfig` factories that overlap in 12 fields. The second is a
stripped-down variant with hardcoded defaults
(`allow_login_shell: false`, `pty: PtyMode::None`, `sandbox: None`). These
hardcoded values are policy decisions that have leaked from the
port-forward startup path into the config module. Move the defaults next to
the port-forward setup, or build both via a `Default` + `with_*`
chain.

### 6.4 `LocalDaemonClient` mirrors `DaemonClient` without a shared trait

`crates/remote-exec-broker/src/local_backend.rs:28-73` and
`crates/remote-exec-broker/src/daemon_client.rs` expose identical method
signatures (`target_info`, `exec_start`, `exec_write`, `patch_apply`,
`image_read`). The macro-based `dispatch_backend!` in `target/handle.rs`
exists precisely because there is no shared trait. Adding a
`DaemonBackend` trait would replace the macro with a plain trait object or
enum dispatch, and make the two implementations type-checked symmetrical.

### 6.5 `decode_rpc_error_strict` / `decode_rpc_error_lenient` near-identical

`daemon_client.rs:822-836` has two functions that both read
`response.text().await` and call `decode_rpc_error_body`. The only
difference is whether a text-read error becomes `Transport(err)` or feeds
into `decode_rpc_error_body` as the body. A single function with a
`OnReadError` enum would remove the duplication.

### 6.6 `preview_text` triple-hop

`remote-exec-util` exposes `preview_text`. `remote-exec-host/src/logging.rs`
re-wraps it. `remote-exec-broker/src/logging.rs` re-wraps it again. C++
re-implements it in `crates/remote-exec-daemon-cpp/src/logging.cpp:188`. A
2-line truncation helper does not justify its own crate plus two shim
files.

### 6.7 Test frame construction by `serde_json::json!`

`tcp_bridge.rs` builds tunnel test frames via
`serde_json::to_vec(&serde_json::json!({...}))` 11 times;
`udp_bridge.rs` does the same several times. `encode_tunnel_meta` already
exists for production code with typed structs (`TcpAcceptMeta`,
`UdpDatagramMeta`). Reusing the typed encoder in tests catches schema
drift; the current pattern silently tolerates it.

### 6.8 `remote-exec-util` is a 40-line crate

`crates/remote-exec-util/src/lib.rs` exports two functions:
`init_compact_stderr_logging` (12 lines) and `preview_text` (7 lines). Both
have closer homes — the logging init in each binary's `main.rs`, and
`preview_text` in `remote-exec-proto` (which has no binary deps) or
inlined. The crate is one of nine workspace members; a workspace split
should justify itself.

---

## 7. Long functions and deep nesting (MEDIUM / LOW)

The places where a function does multiple things and the reader has to
maintain the call stack mentally. None are bugs; all are readability cost.

- `crates/remote-exec-host/src/port_forward/tcp.rs:186` —
  `tunnel_tcp_accept_loop` is ~100 lines of `loop { tokio::select! { match
  accepted { match permit { match frame { … } } } } }`. Extract
  `handle_accepted_tcp_stream` and `acquire_stream_permit`.
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:180-248` —
  `handle_listen_tcp_accept` is ~69 lines, 5 levels deep, with a
  stream-ID exhaustion path (lines 194–200) that triggers a full epoch
  recovery inline.
- `crates/remote-exec-broker/src/port_forward/supervisor/open.rs:207-322` —
  `build_opened_forward` is 116 lines combining store registration, listen
  frame send, listener-ready ack wait, and `OpenedForward` build. Split at
  the store-registration boundary.
- `crates/remote-exec-host/src/exec/shell/windows.rs:49` —
  `resolve_default_windows_shell_with_validator` is ~65 lines of nested
  fallback chains (`COMSPEC` → `cmd.exe` → Git Bash → …). An iterator of
  candidates passed to a single resolver would flatten this.

---

## 8. Error handling inconsistencies (MEDIUM / LOW)

### 8.1 Anyhow at the boundaries with typed errors underneath

`crates/remote-exec-broker/src/port_forward/tunnel.rs:38` defines a
`thiserror`-derived `TunnelError`, but most tunnel functions return
`anyhow::Result`. The typed error is converted to `anyhow::Error` at the
construction site, then `is_retryable_transport_error` /
`is_backpressure_error` (called from `tcp_bridge.rs:213`,
`reconnect.rs:294`, etc.) recover the type by string-matching or
downcasting. Either commit to `TunnelError` end-to-end or remove it.

`remote-exec-host/src/exec/handlers.rs` shows the same pattern: returns
`Result<_, HostRpcError>` but maps `anyhow::Result` chains via
`.map_err(internal_error)`, dropping any `.context(...)` chain at the
boundary.

### 8.2 `map_winpty_error` discards the structured error

`crates/remote-exec-host/src/exec/winpty.rs:20-22`:

```rust
fn map_winpty_error(err: winptyrs::Error) -> anyhow::Error {
    anyhow::anyhow!(err.to_string())
}
```

`anyhow::Error::new(err)` (or a `#[from]` impl on a typed error) preserves
the source chain and allows downcasting. The current shape forces every
caller to string-match if it cares.

### 8.3 `TunnelErrorMeta` decoded via raw `serde_json::Value`

`crates/remote-exec-broker/src/port_forward/tunnel.rs:422-443` extracts
fields by `value.get("code").and_then(|c| c.as_str())` rather than
`serde_json::from_slice::<TunnelErrorMeta>(&frame.meta)` followed by a
fallback. The typed struct already exists; the manual extraction adds
boilerplate and lets schema drift go unnoticed.

A related smell: `format_terminal_tunnel_error` at lines 455–463 detects
the synthetic "fallback" error by string-equality on a message that another
function in the same module just constructed
(`format!("port tunnel returned error on stream {}", meta.stream_id)`). A
typed sentinel (e.g. an `Option<KnownTunnelErrorCode>`) is more honest.

### 8.4 Wire codes as bare string literals

`reconnect.rs:323,336` and `open.rs:534` pass `"operator_close"` and
`"listener_open_failed"` as raw `&str`. These are wire-protocol identifiers.
Promote them to `pub const` in `remote-exec-proto` so typos fail to
compile and grep finds the producers.

### 8.5 Bare `catch (...)` in C++ tunnel paths

13 occurrences across `port_tunnel.cpp`, `port_tunnel_sender.cpp`,
`port_tunnel_tcp.cpp`, `port_tunnel_udp.cpp`, `port_tunnel_session.cpp`,
`port_forward_socket_ops.cpp`, `process_session_posix.cpp`. Most have no
logging in the catch arm. The three nested catch-alls in
`port_tunnel.cpp:163,193,203` sit inside the dispatch loop; a logic error
in frame handling would be silently swallowed and the loop would continue
with corrupted state.

Mitigation: log at minimum (`std::current_exception()` →
`std::rethrow_exception` and capture `what()` in a typed wrapper), or
narrow the catch to the specific exception types thrown by the call.

### 8.6 `unwrap_or` silently maps invalid status codes to 500

`crates/remote-exec-daemon/src/rpc_error.rs:13`:

```rust
StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
```

If a `HostRpcError` returns a status code outside HTTP range, the daemon
silently maps it to 500 with no `tracing::warn!`. A log at
`tracing::error!` level would surface programming errors that today
disappear.

### 8.7 Win32 socket errors as raw integers

`crates/remote-exec-daemon-cpp/src/server_transport.cpp:38-41` formats
Win32 socket errors as the bare integer code while the POSIX path runs
through `std::strerror`. `FormatMessageA` (Windows-XP-compatible) would
match POSIX readability.

---

## 9. Naming and module-structure inconsistencies (LOW)

### 9.1 `local_backend.rs`, `local_port_backend.rs`, `local_transfer.rs`

Three top-level broker modules for the local-target path.
`local/{backend,port,transfer}.rs` would group them. `local_backend.rs` is
also a likely home for a future `DaemonBackend` trait (see §6.4).

### 9.2 `supervisor/open.rs` exports `open_listen_session` / `open_data_tunnel`

Used by `reconnect.rs`. The file is named for the open-a-new-forward
operation; tunnel-open primitives shared across open/reconnect should sit
in `tunnel_open.rs` or back in `tunnel.rs`. Today, finding "where do
tunnels actually get opened" requires reading two files in the
`supervisor/` directory plus the parent.

### 9.3 `exec/support.rs` is a grab-bag

Holds `resolve_workdir`, `ensure_sandbox_access`, and the `poll_until_exit`
loop. The name conveys nothing about what's inside. Either fold into
`handlers.rs` (the only caller of two of the three) or rename to
`exec/policy.rs`.

### 9.4 `port_forward/active.rs` mixes state with access control

Contains both the data structs (`ActiveTunnelState`, `ListenContext`,
`ConnectContext`) and the access-control helpers (`require_protocol`,
`require_listen_session`, `require_connect_tunnel`). The access methods
read like a separate concern; an `access.rs` sibling would clarify.

### 9.5 Constant-naming styles mixed in C++

`process_session_posix.cpp:29-32` — `kCamelCase`
(`kDefaultPtyRows`, `kTerminateGraceMs`).
`session_store.cpp:42-43` — `SCREAMING_SNAKE`
(`EXIT_POLL_INTERVAL_MS`, `RECENT_PROTECTION_COUNT`).
`port_tunnel.cpp:28` — `SCREAMING_SNAKE` (`READ_BUF_SIZE`).
A `.clang-format` already exists; add a constant-naming entry to a style
guide and converge.

### 9.6 Include-guard inconsistency

`crates/remote-exec-daemon-cpp/include/path_utils.h` uses `#ifndef
REMOTE_EXEC_PATH_UTILS_H`. Every other production header in the directory
uses `#pragma once`. Two test headers
(`tests/test_filesystem.h`, `tests/test_text_file.h`) also use `#ifndef`.
Pick one and apply.

### 9.7 `tls.rs` carries non-TLS HTTP helpers

`crates/remote-exec-daemon/src/tls.rs` is a feature-flag dispatcher, but it
also exposes `bind_listener` and `serve_http*`. Those belong in
`http_serve.rs` or `server.rs`.

### 9.8 `target/backend.rs` is a 5-line enum with only one consumer

The macro `dispatch_backend!` in `target/handle.rs:15-22` exists because
the enum has no shared interface. Either give it a trait (see §6.4) or
collapse it back into a single concrete dispatcher.

---

## 10. Vendored / dead code (LOW)

### 10.1 `crates/remote-exec-daemon-cpp/third_party/httplib.h` — 10,352 lines, never `#include`d

Build files put `third_party/` on the include path, but no `.cpp` or `.h`
file outside the vendor file references `httplib::` or `#include
"httplib.h"`. The daemon implements its own HTTP stack via
`http_codec.cpp`, `http_connection.cpp`, `server_transport.cpp`.

`httplib.h` is one of the larger files in the repo. Removing it (or
deleting the include-path entry) trims compile-time include search and
removes a maintenance liability. If it's reserved for future use, leave a
`README.md` in `third_party/` saying so.

---

## 11. Magic numbers (LOW)

- `crates/remote-exec-host/src/exec/winpty.rs:35,37` — `PtySize::new(120,
  24)` and `timeout_ms(10_000)` unnamed. `connect_timeout_ms` default is
  also 10_000 in `daemon/src/config/mod.rs:106` — coincidence or
  duplication?
- `crates/remote-exec-host/src/exec/session/spawn.rs:158` —
  `let mut buffer = [0u8; 8192];` while `port_forward/mod.rs` and the C++
  daemon use 64 KiB. Inconsistent.
- `crates/remote-exec-host/src/exec/session/live.rs:8` —
  `TRANSCRIPT_LIMIT_BYTES = 1024 * 1024` is named but not exposed in
  config; impossible to tune at deploy time.
- C++ `char buffer[4096]` repeats four times across `server_transport.cpp`
  and `process_session_posix.cpp`.

---

## 12. Findings rejected during verification

The subagents flagged several `unwrap()/expect()` panics in "production"
paths that turned out to be inside `#[cfg(test)]` modules. Listed here so
the rejection is auditable:

- `crates/remote-exec-host/src/state.rs:155,159` — inside
  `#[cfg(test)] mod tests` (line 123).
- `crates/remote-exec-broker/src/broker_tls.rs:49` — inside
  `#[cfg(test)] mod tests`.
- `crates/remote-exec-admin/src/bootstrap.rs:142,147,173,178` — inside a
  test function (the `#[test] fn` starts at line 135).
- `crates/remote-exec-broker/src/tools/registry.rs:59` — inside the
  `#[cfg(test)] mod tests` block.
- `crates/remote-exec-broker/src/port_forward/test_support.rs` — flagged
  as not `cfg(test)`-gated, but `port_forward/mod.rs:10-11` does
  `#[cfg(test)] mod test_support;`.

The `unreachable!` in `bin/remote_exec.rs:656` is reachable only if
`clap`'s mutual-exclusion check fails; treating it as a structural smell
is overkill, and it's been left out of the report.

---

## How to sequence the work

These are independent and can be picked in any order, but cheap-but-painful
items first usually pay off the soonest.

1. **Cheap, mechanical, no risk** —
   - Delete `host_path_policy()` in favour of the proto re-export (§6.1).
   - Delete the argument-flipping `path_is_within` wrapper (§6.2).
   - Inline `wire.rs` and collapse `sandbox/` (§3.6, §3.7).
   - Convert `"operator_close"` etc. to `pub const` in proto (§8.4).
   - Delete unused `httplib.h` or note it (§10.1).
   - Standardise C++ include guards on `#pragma once` (§9.6).

2. **Mid-risk file splits, no behaviour change** —
   - Split `tcp_bridge.rs`, `daemon_client.rs`, `config.rs`,
     `bin/remote_exec.rs`, `rpc/transfer.rs`, `public.rs` (§2).
   - Move `port_tunnel_tests.rs` out of `src/` (§2.5).
   - Split `basic_mutex.cpp` and `server_transport.cpp` into platform
     siblings (§4.1, §4.2).

3. **Contract work, needs review** —
   - Add fixture-driven conformance tests for path policy, sandbox rules,
     and port-tunnel frames so the C++ daemon and Rust proto cannot
     silently drift (§1).
   - Drop redundant `ForwardPortsAction` (§3.2).
   - Introduce `DaemonBackend` trait, remove `dispatch_backend!` macro
     (§6.4, §9.8).
   - Decide on `anyhow` vs typed errors for `TunnelError` and stop
     string-matching across the boundary (§8.1, §8.3).

Items in tier 3 have public-surface or wire-protocol implications and
should be batched with releases that already break compatibility.
