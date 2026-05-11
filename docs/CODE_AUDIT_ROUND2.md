# remote-exec-mcp code audit — round 2

Follow-up audit after the first round of remediation commits landed. Items are grouped by theme: (A) correctness and security bugs, (B) incomplete refactors left over from round 1, (C) remaining structural smells. References use `file:line`.

## A. Correctness and security bugs (new)

### 1. UTF-8 boundary splitting in pipe reader

`crates/remote-exec-host/src/exec/session/spawn.rs:119`

```rust
.send(String::from_utf8_lossy(&buffer[..read]).into_owned())
```

Fixed 8 KB buffer, `from_utf8_lossy` called per raw `read()`. When a multi-byte codepoint straddles a buffer boundary, both halves produce `\u{FFFD}`. Any CJK / emoji / non-ASCII stdout is silently corrupted.

Fix: keep a carry buffer of incomplete tail bytes between reads; only emit complete codepoints.

### 2. Unbounded `read_to_end` per archive entry

`crates/remote-exec-host/src/transfer/archive/import.rs:399-400`

```rust
let mut bytes = Vec::new();
std::io::Read::read_to_end(entry, &mut bytes)?;
```

Every tar entry is fully buffered into a `Vec<u8>` before disk write. No per-entry or archive size cap anywhere in the transfer pipeline (checked `export.rs`, `import.rs`, `operations.rs`, `local_transfer.rs`). A 10 GB entry OOMs the daemon.

Fix: `std::io::copy` to the output file, plus a `MAX_ARCHIVE_BYTES` / `MAX_ENTRY_BYTES` config value enforced in both Rust and C++ paths.

### 3. Heap overflow on 32-bit from untrusted size field (C++)

`crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp:184` and `transfer_ops_tar.cpp:347`

```cpp
std::string body(static_cast<std::size_t>(size), '\0');
```

`size` is `uint64_t` from the wire; cast to `std::size_t` with no range check. On a 32-bit target (and the XP build is 32-bit) a `size > UINT32_MAX` truncates silently, then the following `read_exact_or_throw` writes the full declared size into the undersized allocation.

Fix: `if (size > std::string::max_size()) throw …;` before each cast; same guard in `tar.cpp:347`.

### 4. Symlink target not validated on import (C++)

`crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp:178-201` (`write_symlink`)

Link target is taken from `header.link_name` and passed straight to the OS. No check for absolute paths, `..` components, or drive letters. A crafted archive plants a symlink to `/etc/passwd` or `..\..\Windows\System32\…`. The Rust side enforces `TransferSymlinkMode`; the C++ side does not mirror this for the link target.

Fix: reject absolute targets and `..` components; apply sandbox policy to the resolved target, not just the entry path.

### 5. Patch engine: no atomic multi-file apply

*Important Developer Note: This one is the intential behavior of the product, don't make apply patch transactional, but document it clearly it is intentionally non transactional*

`crates/remote-exec-host/src/patch/mod.rs:56-106` and mirrored at `crates/remote-exec-daemon-cpp/src/patch_engine.cpp:576-622`

Actions are applied sequentially with direct `tokio::fs::write` / `std::rename`. If action N+1 fails, actions 0..N are already on disk, and the error returned gives no machine-readable list of which files changed.

Fix: write each file to a sibling `.tmp`, collect the pairs, rename atomically only after all actions succeed; on failure, unlink the temp files and report unchanged state.

### 6. Lock held across network I/O in port-forward supervisor

`crates/remote-exec-broker/src/port_forward/supervisor.rs:769-771`

```rust
let mut state = control.state.lock().await;
let tunnel = resume_listen_session_inner(control).await?;    // full reconnect under lock
state.current_tunnel = Some(tunnel.clone());
```

`resume_listen_session_inner` does a TCP connect + TunnelOpen/Ready round-trip. Any concurrent `current_tunnel()` blocks for the full reconnect. Same pattern at `supervisor.rs:923` (`close_listen_session` holds the state lock across `resume_listen_session_inner` plus two more awaited network calls).

Fix: release the state lock before the network I/O; re-acquire only to swap `current_tunnel` after the tunnel is fully established.

### 7. `PortTunnelConnection::run()` catches only `std::exception`

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:549-568`

Dispatch loop wraps in `catch (const std::exception&)`. A non-standard throw (or a future exception type that doesn't inherit from `std::exception`) skips `close_current_session` / `close_transport_owned_state`, leaking the session budget and orphaning TCP/UDP streams.

Fix: add `catch(...)` after the `std::exception` arm performing the same teardown.

### 8. Silent drop of `TcpEof` / `Shutdown` under queue pressure

`crates/remote-exec-host/src/port_forward/tcp.rs:592,609`

```rust
if writer.tx.try_send(TcpWriteCommand::Shutdown).is_ok() { … clear cancel … }
```

`try_send` on a cap-8 channel. If full, the half-close is silently dropped, the cancel token is never cleared, and the remote never sees EOF. Same failure mode for the heartbeat ACK at `broker/tunnel.rs:117` (cap-128 channel): under sustained data pressure, dropped ACKs time out the tunnel.

Fix: either `send` with a short timeout, or reserve a dedicated small control-frame lane that's not subject to data backpressure.

### 9. TOCTOU on tunnel mode assignment (host)

`crates/remote-exec-host/src/port_forward/tunnel.rs:192,268`

```rust
if !matches!(*tunnel.open_mode.lock().await, TunnelMode::Unopened) { … }
// … await …
*tunnel.open_mode.lock().await = TunnelMode::Listen { … };
```

Two lock acquisitions with `await`s between them. Concurrent `TunnelOpen` frames can both pass the "Unopened" check before either writes.

Fix: take the lock once, check and swap in place under the same guard; model as a proper `compare_exchange`.

### 10. `close_active_tcp_listen_streams` leaks slots on double failure

`crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:533-540`

If `listen_tunnel.close_stream` fails partway through, the function returns early without calling `record_dropped_streams_and_release_active`. Remaining streams in the iterator never release their active-stream budget.

Fix: accumulate the failure, continue releasing, surface the first error at the end.

### 11. `schedule_session_expiry` is a detached spawn that can race reconnects

`crates/remote-exec-host/src/port_forward/session.rs:251`

Spawned task sleeps for `resume_timeout` then expires. `JoinHandle` is discarded. If `close_attached_session` fires again for the same session, two timers are in flight; the older one can fire against a freshly re-attached session.

Fix: store the handle on the session and abort-on-reattach, or use a cancellation token the new attachment toggles.

### 12. Private-key file permissions on Windows (follow-up)

`crates/remote-exec-pki/src/write.rs:207,221-235`

Partially addressed by `harden pki file writes`. Verify: on non-Unix, private-key files still land with default ACLs — there is no Windows equivalent to the Unix mode apply. Either add an ACL path or document the limitation explicitly.

## B. Incomplete refactors (round-1 holdouts)

### 13. `"port_tunnel_limit_exceeded"` string literals still in broker

Four holdouts after the `ErrorCode` refactor:
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:1564` (inside a `json!` literal)
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:1625` (`reason: "…".to_string()`)
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs:635` (json literal)
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs:696` (reason string)

Fix: use `RpcErrorCode::PortTunnelLimitExceeded.wire_value()`.

### 14. `TransferCompression` left behind in `rpc.rs`

`crates/remote-exec-proto/src/rpc.rs:140` — the other three transfer enums moved to `transfer.rs`, but `TransferCompression` stayed, forcing `transfer.rs:92,94,104,113,124` to reach back with `crate::rpc::TransferCompression`.

Fix: move it and delete the back-edge.

### 15. Two `rpc_error` helpers still coexist

- `crates/remote-exec-host/src/exec/support.rs:44`
- `crates/remote-exec-host/src/port_forward/error.rs:11`

Identical bodies. The "unify host rpc error mapping" commit consolidated error *mapping* but not this constructor helper.

Fix: single `pub(crate) fn rpc_error` in `host::error`.

### 16. `PortTunnelTimings` refactor only applied to host

`crates/remote-exec-broker/src/port_forward/mod.rs:22-41` still defines module-level `PORT_TUNNEL_HEARTBEAT_INTERVAL`, `PORT_TUNNEL_HEARTBEAT_TIMEOUT`, plus `#[cfg(not(test))]`/`#[cfg(test)]` twins and a `REMOTE_EXEC_TEST_*` env-var escape hatch. Host uses a proper `timings()` accessor.

Fix: extend the broker to use the same `PortTunnelTimings` struct and drop the env-var path.

### 17. `test_duration_override` fires in non-test debug builds

`crates/remote-exec-broker/src/port_forward/mod.rs:63-67` reads `REMOTE_EXEC_TEST_PORT_TUNNEL_HEARTBEAT_TIMEOUT_MS` whenever `debug_assertions` is on, not only under `#[cfg(test)]`. A debug release build shipped to an operator picks this up from the environment.

Fix: gate by `cfg(test)` or behind a compile-time feature.

### 18. `sse_*_ms: Option<u64>` with `Some(0)` as a disable sentinel

`crates/remote-exec-broker/src/config.rs:43-46` + `mcp_server.rs:270-276` silently converts `Some(0)` to `None`. TOML type and semantic type disagree.

Fix: custom `Deserialize` into `Option<Duration>`, or a named `SseInterval` newtype.

### 19. Sentinel-zero for `port_forward_protocol_version` not tightened

`crates/remote-exec-proto/src/rpc.rs:31` — still `u32` with `#[serde(default)]` and `0` meaning "unsupported"; `ListTargetDaemonInfo` mirrors this.

Fix: `Option<NonZeroU32>` end-to-end; dedicated `SupportedProtocolVersion` type.

### 20. `ExecResponse.daemon_session_id: Option<String>` invariant untightened

`crates/remote-exec-proto/src/rpc.rs:62` — `ExecWriteRequest` requires the session ID; `ExecResponse` makes it optional even when a PTY session exists.

Fix: split into `ExecStartResponse` (never optional) and `ExecCompletedResponse`.

### 21. IDs still plain `String`

`ids.rs` centralized *generation* (good) but returns `String`. Call sites at `host/src/exec/handlers.rs:239`, `host/src/state.rs:84`, `host/src/port_forward/tunnel.rs:316`, `broker/src/session_store.rs:28`, `broker/src/port_forward/supervisor.rs:383` store them as `String`. A `forward_id` and a `session_id` are freely swappable at the type level.

Fix: introduce `SessionId`, `InstanceId`, `ForwardId`, `PortTunnelId` newtypes with a shared macro.

### 22. `port_forward.rs` proto still returns `anyhow::Result`

`crates/remote-exec-proto/src/port_forward.rs` uses `anyhow::Result` for `normalize_endpoint`, `ensure_nonzero_connect_endpoint`, `endpoint_port`. Only remaining proto module without a typed error.

Fix: introduce `PortForwardProtoError` with `thiserror`.

### 23. `platform::join_path` survived the C++ path consolidation

`crates/remote-exec-daemon-cpp/src/platform.cpp:141` vs `path_utils::join_path` (`path_utils.cpp:21`). On Windows, `platform::join_path` normalizes `/` to `\`; `path_utils::join_path` does not. `shell_policy.cpp:98` still calls the platform version.

Fix: remove `platform::join_path` or make it delegate; update the caller.

### 24. `patch_engine.cpp` has its own `make_directory_if_missing` / `create_parent_directories`

`crates/remote-exec-daemon-cpp/src/patch_engine.cpp:194-230` — full reimplementation of the pair that lives in `transfer_ops_fs.cpp:121-166`.

Fix: call the shared helpers from `path_utils`.

### 25. `DaemonConfig.port_forward_max_worker_threads` is a redundant top-level field

`crates/remote-exec-daemon-cpp/include/config.h:42` and `src/config.cpp:428` set it from `config.port_forward_limits.max_worker_threads`. Only tests read it.

Fix: delete the top-level field; tests use the nested one.

## C. Remaining structural smells

### 26. Large files still large after decomposition

- `crates/remote-exec-proto/src/rpc.rs` (840 LOC) — exec DTOs, leftover `TransferCompression`, image DTOs, patch DTOs, error types, header parsing, tests. Natural split: `rpc/error.rs`, `rpc/exec.rs`, `rpc/transfer.rs`, `rpc/patch.rs`.
- `crates/remote-exec-daemon-cpp/src/session_store.cpp` (910 LOC) — ID generation, output pump (POSIX + Win32 variants), drain/wait, prune/eviction, pending-start reservation, and both public RPC methods. Extract `SessionPump` class.

### 27. No per-RPC timeout on broker → daemon calls

`crates/remote-exec-broker/src/daemon_client.rs:428-467` — `post()` has no `tokio::time::timeout`; the `reqwest::Client` at `:612-614` has no `.timeout()` either. Only the port-tunnel upgrade path has explicit timeouts. A hung daemon blocks startup probes (`startup.rs:76,105`) and every tool call indefinitely.

Fix: thread a request-level timeout through config; set `reqwest` connect/read defaults.

### 28. Startup probe is serial and unbounded

`crates/remote-exec-broker/src/startup.rs:89-98` iterates targets serially, awaiting `target_info()` with no timeout. N slow targets = N × stall.

Fix: `futures::future::join_all` with per-target timeout; treat timeout as "offline" rather than startup failure.

### 29. Duplicated `HttpAuthConfig` shape in broker and daemon

- `crates/remote-exec-broker/src/config.rs:71-72`
- `crates/remote-exec-daemon/src/config/mod.rs:59-62`

Same field (`bearer_token`), same validation body. The daemon caches `expected_authorization`; the broker caches it in `build_bearer_authorization_header`.

Fix: single `HttpAuthConfig` in `proto` exposing the pre-computed header as a method.

### 30. Tunnel queue budget duplicated in three places

`8 * 1024 * 1024` hard-coded at:
- `crates/remote-exec-broker/src/port_forward/tunnel.rs:37` (`PortTunnel::DEFAULT_MAX_QUEUED_BYTES`)
- `crates/remote-exec-broker/src/port_forward/limits.rs:27` (`BrokerPortForwardLimits::default`)
- `crates/remote-exec-host/src/config/mod.rs:81` (`HostPortForwardLimits::default`)

Fix: single `const` in `proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES`.

### 31. Ad-hoc JSON key access in exec logging

`crates/remote-exec-broker/src/tools/exec.rs:113-114`

```rust
running = structured["session_id"].is_string(),
exit_code = structured["exit_code"].as_i64().unwrap_or(-1),
```

Only site of string-key access; silently logs wrong values if field names change.

Fix: deserialize into `CommandToolResult` for logging.

### 32. Magic poll intervals not unified

`handlers.rs` defines `EXEC_START_POLL_INTERVAL_MS = 25`, but `support.rs:89` and `session/windows.rs:356,394` have their own inline `25` / `300` literals.

Fix: use the named constant.

### 33. Scattered `"local"` string match in transfer routing

`crates/remote-exec-broker/src/tools/transfer/operations.rs:39-157` matches on `(source.target.as_str(), destination.target.as_str())`; `endpoints.rs:47,231` also string-compares `== "local"`. The exec path uses `TargetBackend::Local` enum dispatch.

Fix: apply the same enum refactor to transfer.

### 34. `TargetHandle` file-based transfer returns a misleading error for local

`crates/remote-exec-broker/src/target/handle.rs:143,176,187` — `transfer_export_to_file`, `transfer_import_from_file`, `transfer_import_from_body` return `unsupported_local_transfer_error()` for `TargetBackend::Local(_)`. Callers in `operations.rs` route around the handle for local, so this is effectively dead code — but a trap for future callers using the handle uniformly.

Fix: delegate to `local_transfer` from the handle, or remove these methods from the local backend so the type system makes the routing explicit.

### 35. `call_direct_tool` dispatch table duplicates MCP registration

`crates/remote-exec-broker/src/client.rs:187-200` — tool names match `#[tool_router]` in `mcp_server.rs` by hand. No compile-time check.

Fix: drive both from the same `Tool::NAMES` constant or an enum.

### 36. `expect` still in production paths

- `crates/remote-exec-broker/src/client.rs:226`: `serde_json::to_value(content).expect("serializing raw MCP content")` on the hot path.
- `crates/remote-exec-broker/src/local_port_backend.rs:32`: `std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir())` — silent fallback to `/tmp`, no log.

### 37. `HTTP_CONNECTION_IDLE_TIMEOUT_MS` not in config (C++)

`crates/remote-exec-daemon-cpp/src/http_connection.cpp:22` hard-codes `30000UL`. All other timeouts are in `config.h` as `DEFAULT_*` constants.

Fix: hoist into `DaemonConfig`.

### 38. `Internal` wire-code asymmetry untested

`crates/remote-exec-proto/src/rpc.rs:537,585`: `wire_value()` emits `"internal_error"`, but `from_wire_value` accepts both `"internal"` and `"internal_error"`. No test covers the alias or forward-compat for unknown codes (`_ => None` arm).

Fix: add a round-trip test and an unknown-code test asserting `None`.

### 39. `pump_session_output` swallows exceptions silently (C++)

`crates/remote-exec-daemon-cpp/src/session_store.cpp:283-291` — `catch (const std::exception&)` retires the session with no log. Any read error leaves no diagnostic.

Fix: log at WARN including `e.what()` before retiring.

### 40. UDP connector sweep holds inner mutex across full iteration

`crates/remote-exec-host/src/port_forward/udp_connectors.rs:58-74` — `sweep_idle` collects, removes, and returns under one lock. `close_stream` is called outside the lock (correct), but the sweep critical section scales with map size.

Fix: snapshot expired keys under lock, then remove in a second short critical section.

## Staged plan

### Phase A — correctness and security (must ship first)

Items 1–11 are real bugs. Three of them (#2, #3, #4) can be triggered by a hostile archive; #6 is a deadlock-class issue on reconnect. Start here and add regression tests for each failure mode (malicious archive, mid-reconnect `current_tunnel()`, pipe boundary on CJK).

The single most-urgent item is **#2 + #3 combined** — unbounded allocation on untrusted input on both Rust and C++ daemons with no size cap in the proto or config. Introduce a `TransferLimits` struct in proto and enforce it at the archive boundary on both sides in the same PR.

### Phase B — finish the prior-round refactors

Items 13–25. Mechanical holdouts. Doing them alongside Phase A keeps typed errors / typed codes / typed IDs covering every call site, rather than "most of them," which is how they erode.

### Phase C — structural and operational polish

Items 27, 28, 29, 33, 37 are the ones that will bite an operator at 3 a.m. (no timeouts, serial startup, scattered routing). Items 26, 34, 35 are refactors that can wait but earn their keep the next time you touch that area. The rest (#30, #31, #32, #36, #38, #39, #40) are ~30-minute cleanups; bundle them.
