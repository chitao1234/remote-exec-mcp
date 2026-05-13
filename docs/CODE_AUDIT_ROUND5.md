# Code Audit — Round 5

Focus: implementation quality (duplication, god modules, unclear boundaries,
ad-hoc patterns). Metrics feature remains postponed. Non-atomic patch apply is
intentional product behavior.

---

## Round 4 Fix Verification

| R4 # | Title | Status |
|-------|-------|--------|
| 1 | `port_tunnel_transport.cpp` 931 LOC | Fixed (352 LOC now) |
| 6 | `archive_error_to_transfer_error` duplication | Fixed (shared import) |
| 9 | `"local"` magic string | Fixed (`LOCAL_TARGET_NAME` constant) |
| 15 | Wire structs duplicated broker/host | Fixed (live in proto) |
| 17 | `TunnelRole` defined twice | Fixed (proto canonical, broker re-exports) |
| 21 | `logging.rs` duplicated across 3 crates | Partial — broker uses `remote_exec_util`, host/daemon still have own copies |
| 12 | `ScriptedTunnelIo` test duplication | Unfixed (still in tcp_bridge.rs + udp_bridge.rs) |
| 20 | `write_ca_pair`/`write_broker_pair`/`write_daemon_pair` | Unfixed (714 LOC write.rs) |
| 34/35 | Pair teardown duplication in tcp_bridge | Unfixed (1669 LOC) |

---

## New Findings

### A. Byte-for-Byte Duplication (Items 1–7)

**1. `yield_time.rs` duplicated across host and daemon (164 LOC each)**

- `crates/remote-exec-host/src/config/yield_time.rs`
- `crates/remote-exec-daemon/src/config/yield_time.rs`

`diff` returns empty — identical files. Any change must be applied twice.

Fix: have daemon re-export from host, or move to proto/shared crate.

---

**2. `send_tunnel_error` — four near-identical functions**

- `crates/remote-exec-host/src/port_forward/tunnel.rs` — `send_tunnel_error()`, `send_tunnel_error_code()`
- `crates/remote-exec-host/src/port_forward/session.rs` — `send_tunnel_error_with_sender()`, `send_tunnel_error_code_with_sender()`

Each pair differs only in whether it takes `RpcErrorCode` or `String`, and
whether it takes `TunnelState` or `TunnelSender`. 24+ call sites must choose
the correct variant.

Fix: introduce a `TunnelSender` trait, single `send_error(sender, impl
Into<String>)` function.

---

**3. `classify_recoverable_tunnel_event` match block — 4 copies**

- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs` (2 copies, listen + connect)
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs` (2 copies)

Each is a 12-line match that classifies a frame result into Frame /
RetryableTransportLoss / TerminalTransportError / TerminalTunnelError. Only the
context string differs.

Fix: extract `recv_or_recover(tunnel, role, context) -> Result<Frame,
ForwardLoopControl>`.

---

**4. `ensure_*_success` — three near-identical methods in daemon_client.rs**

- `ensure_transfer_export_success` (lines 322–341)
- `ensure_transfer_import_success` (lines 405–424)
- `ensure_rpc_success` (lines 529–548)

All check `response.status().is_success()`, log a warning with
target/url/elapsed, then call `decode_rpc_error`. Only the log text differs.

Fix: single `ensure_success(response, operation_name)` with a context struct.

---

**5. `run_tcp_forward` / `run_udp_forward` outer recovery loop**

- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:38–65`
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs:20–48`

Identical structure: acquire tunnels → loop { run epoch → match
ForwardLoopControl → recover }. UDP adds one `record_dropped_datagram()` call.

Fix: extract generic `run_forward_loop(runtime, epoch_fn)` with a pre-recovery
hook.

---

**6. `transfer_single_source` — 4 near-identical match arms**

- `crates/remote-exec-broker/src/tools/transfer/operations.rs:38–166`

Matches `(source_type, dest_type)` producing 4 arms (L→L, L→R, R→L, R→R).
`build_export_request` and `build_import_request` calls are identical in all
arms; only the export/import dispatch (local vs daemon) varies.

Fix: factor into `export_to_reader(source)` + `import_from_reader(dest,
reader)` — collapses 4 arms to 2 sequential calls, removes ~80 lines.

---

**7. Session teardown pattern — 4 copies in C++ session_store.cpp**

- `crates/remote-exec-daemon-cpp/src/session_store.cpp:188–194, 332–339, 406–411, 501–507`

```cpp
{
    BasicLockGuard session_lock(session->mutex_);
    session->retired = true;
    session->closing = true;
    session->cond_.broadcast();
    if (session->process.get() != NULL) { session->process->terminate(); }
}
join_session_pump(session.get());
```

Fix: extract `retire_session(LiveSession*)`.

---

### B. God Modules (Items 8–14)

**8. `tcp_bridge.rs` — 1669 LOC**

- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`

Production code ~800 LOC + ~870 LOC tests. The "send-then-classify-error"
pattern (lines 583–605, 646–672, 437–451) is repeated 3 times with minor
variations in close-pair vs close-all action.

Fix: extract `send_or_classify(tunnel, frame, context) ->
Result<(), SendFailureAction>` where `SendFailureAction` is
`{Backpressure, Retryable, Fatal}`.

---

**9. `supervisor.rs` — 1055 LOC, 6 concerns**

- `crates/remote-exec-broker/src/port_forward/supervisor.rs`

Mixes: ForwardRuntime struct + accounting, ListenSessionControl state machine,
open-forward orchestration, tunnel handshake, reconnect retry loop, close/drain.

Fix: split into `supervisor/open.rs` (orchestration + handshake) and
`supervisor/reconnect.rs` (retry, recover, drain). Keep `ForwardRuntime` and
`ListenSessionControl` in `supervisor/mod.rs`.

---

**10. `host/port_forward/mod.rs` — 1468 LOC (1393 are tests)**

- `crates/remote-exec-host/src/port_forward/mod.rs`

75 lines of production code + 1393 lines of integration tests with 40+ test
functions and significant setup duplication (inline `serde_json::json!`
literals, repeated `spawn_tcp_echo_server` calls).

Fix: move tests to `crates/remote-exec-host/src/port_forward/tests.rs` or
`tests/` directory. Extract test helpers (`spawn_tcp_echo_server`,
`spawn_tcp_hold_server`, `spawn_tcp_non_draining_server`) to a shared
`test_support` module.

---

**11. `host/port_forward/tcp.rs` — 662 LOC, dual ownership model**

- `crates/remote-exec-host/src/port_forward/tcp.rs`

Two parallel implementations:
- `tunnel_tcp_read_loop_transport_owned()` (line 288)
- `tunnel_tcp_read_loop_session_owned()` (line 356)

90% identical but differ in sender type (`TunnelState` vs `AttachmentState`).
Bug fixes must be applied twice.

Fix: introduce trait `TunnelSender` to abstract over both, allowing single
implementation.

---

**12. `pki/write.rs` — 714 LOC, 3 concerns**

- `crates/remote-exec-pki/src/write.rs`

Mixes: high-level bundle orchestration (lines 20–76), Unix file writing (lines
194–243), Windows ACL management (lines 267–589 — 322 LOC of Windows API
calls).

Fix: split into `write/bundle.rs`, `write/unix.rs`, `write/windows_acl.rs`.

---

**13. `host/exec/store.rs` — 683 LOC**

- `crates/remote-exec-host/src/exec/store.rs`

Session storage logic (1–306) + test infrastructure (308–683). Platform-specific
test helpers mixed with core logic.

Fix: move tests to separate file.

---

**14. `host/transfer/archive/export.rs` — 532 LOC**

- `crates/remote-exec-host/src/transfer/archive/export.rs`

File export, directory export, archive bundling, and symlink handling all in one
file.

Fix: split into `export_file.rs`, `export_directory.rs`, `export_bundle.rs`.

---

### C. Unclear Boundaries (Items 15–19)

**15. Daemon crate is a thin pass-through**

- `crates/remote-exec-daemon/src/lib.rs`

```rust
pub type AppState = remote_exec_host::HostRuntimeState;
// ...
remote_exec_host::build_runtime_state(config.into())
remote_exec_host::target_info_response(state, ...)
```

The daemon crate adds HTTP routing but almost no independent logic. Its
`config/mod.rs` has empty stubs (`prepare_runtime_fields` is a no-op,
`normalize_configured_workdir` is a pass-through).

Fix: either give daemon clear responsibilities (auth, rate limiting) or merge
config into host. Remove empty stubs.

---

**16. Windows path translation in proto crate**

- `crates/remote-exec-proto/src/path.rs:43–110`

`translate_windows_posix_drive_path()`, `split_windows_prefix()`,
`build_windows_drive_path()` are daemon/broker implementation logic, not wire
protocol definitions.

Fix: move to a `path-utils` module in host or a shared utility crate.

---

**17. `tunnel_mode` dispatch boilerplate — 13+ call sites**

- `crates/remote-exec-host/src/port_forward/tcp.rs`, `udp.rs`

Every operation repeats a 15–20 line match on `tunnel_mode(&tunnel).await`:
```rust
match tunnel_mode(&tunnel).await {
    TunnelMode::Listen { protocol: Tcp, session } => { ... }
    TunnelMode::Connect { protocol: Tcp } => { ... }
    // error cases
}
```

Fix: extract helper methods `with_tcp_session(tunnel, |session| ...)` and
`with_tcp_transport(tunnel, || ...)` that handle the match internally.

---

**18. `TunnelMode::Listen` carries heavy `Arc<SessionState>`**

- `crates/remote-exec-host/src/port_forward/types.rs:81–90`

The `Listen` variant carries `Arc<SessionState>` while `Connect` doesn't. This
asymmetry forces all callers to extract the session from the enum, driving the
repeated dispatch pattern (item 17).

Fix: store session separately in `TunnelState`, use enum only for mode
discrimination.

---

**19. Test infrastructure on production path**

- `crates/remote-exec-broker/src/mcp_server.rs:358` — `write_test_bound_addr_file`
- `crates/remote-exec-broker/src/port_forward/generation.rs:42` — `StreamIdAllocator::set_next_for_test` missing `#[cfg(test)]`

Fix: gate behind `#[cfg(test)]` or move to test-only modules.

---

### D. Ad-hoc Patterns (Items 20–27)

**20. `wire_value()` / `from_wire_value()` boilerplate — 200+ LOC**

- `crates/remote-exec-proto/src/transfer.rs` (4 enums × ~15 lines each)
- `crates/remote-exec-proto/src/rpc/error.rs` (108 variants)
- `crates/remote-exec-proto/src/rpc/warning.rs`

Every enum manually implements the same pattern:
```rust
pub fn wire_value(&self) -> &'static str { match self { ... } }
pub fn from_wire_value(value: &str) -> Option<Self> { match value { ... } }
```

Fix: derive macro or `strum` crate to eliminate boilerplate.

---

**21. Transfer header parsing — closure-based API**

- `crates/remote-exec-proto/src/rpc/transfer.rs:136–258`

Five `parse_required_*()` / `parse_optional_*()` functions follow the same
pattern but are manually written. The closure-based API (lines 138, 146) is
unusual and hard to follow.

Fix: typed header map or builder pattern.

---

**22. `ExecResponse` type sprawl — 5 types for one concept**

- `crates/remote-exec-proto/src/rpc/exec.rs:38–168`

`ExecResponse`, `ExecRunningResponse`, `ExecCompletedResponse`,
`ExecResponseWire`, `ExecOutputResponse` — with custom Serialize/Deserialize
implementations manually converting between them.

Fix: use `#[serde(tag = "...")]` or `#[serde(untagged)]` to eliminate
`ExecResponseWire` and reduce to 2–3 types.

---

**23. Manual frame serialization at every construction site**

- `crates/remote-exec-host/src/port_forward/` — 14+ call sites

Hand-rolled `encode_frame_meta()` / `decode_frame_meta()` at every frame
construction point (tcp.rs: 105, 190, 271, 305, 375; udp.rs: 57, 114, 155,
234, 297; session.rs: 323; tunnel.rs: 254, 285, 326, 409).

Fix: builder pattern for Frame construction that handles serialization
internally: `Frame::tcp_accept(stream_id).encode()`.

---

**24. C++ ostringstream logging — 33 occurrences**

- Throughout `crates/remote-exec-daemon-cpp/src/*.cpp`

```cpp
std::ostringstream message;
message << "text " << variable << " more text";
log_message(LOG_INFO, "component", message.str());
```

Verbose and error-prone (easy to forget `.str()`).

Fix: variadic template helper or format-string wrapper.

---

**25. C++ manual HTTP upgrade response**

- `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:89–92`

Raw string concatenation for HTTP 101 response while other responses use
`render_http_response()`.

Fix: add `render_http_upgrade_response()` for consistency.

---

**26. `HostPortForwardLimits` — 7 unrelated fields**

- `crates/remote-exec-host/src/config/mod.rs:68–78`

Timeout (`connect_timeout_ms`) mixed with capacity limits. No logical grouping.

Fix: split into `CapacityLimits` and `TimeoutConfig` sub-structs.

---

**27. `tunnel_open_listen` — 80-line nested function**

- `crates/remote-exec-host/src/port_forward/tunnel.rs:188–267`

Three levels of nesting: resume existing session → create new session → claim
tunnel mode with rollback → attach and send ready frame. Rollback logic is
interleaved with happy path.

Fix: extract `resume_or_create_session()` and `attach_and_notify()`.

---

### E. Structural (Items 28–30)

**28. `ForwardRuntime` — 10-field bag mixing identity and context**

- `crates/remote-exec-broker/src/port_forward/supervisor.rs:64–76`

Mixes forward identity (id, protocol, endpoints, sides) with runtime context
(store, cancel, limits, tunnels). Cloned into spawned tasks.

Fix: split into `ForwardIdentity` (useful for logging/errors) and
`ForwardRuntime` (identity + operational handles).

---

**29. `sandbox.rs` mixes 3 concerns (401 LOC)**

- `crates/remote-exec-proto/src/sandbox.rs`

Configuration types, path canonicalization, and authorization logic in one file.
`lexical_normalize()` and `path_is_within()` are general utilities.

Fix: split into `sandbox/types.rs`, `sandbox/authorize.rs`,
`sandbox/path_utils.rs`.

---

**30. `DaemonManifestEntry` duplicates `KeyPairPaths` fields**

- `crates/remote-exec-pki/src/manifest.rs:13–23`

```rust
pub struct KeyPairPaths { pub cert_pem: PathBuf, pub key_pem: PathBuf }
pub struct DaemonManifestEntry { pub cert_pem: PathBuf, pub key_pem: PathBuf, pub sans: Vec<...> }
```

Fix: compose — `DaemonManifestEntry { pub paths: KeyPairPaths, pub sans: ... }`.

---

## Staged Remediation Plan

### Phase 1 — Dedup (items 1–7)

Highest ROI. Each item is a self-contained extraction or re-export.

| Item | Effort | LOC saved |
|------|--------|-----------|
| 1 | XS | 164 (delete one file) |
| 7 | XS | ~24 (C++ helper) |
| 4 | S | ~40 |
| 3 | S | ~36 |
| 2 | S | ~30 |
| 5 | M | ~25 |
| 6 | M | ~80 |

### Phase 2 — Split god modules (items 8–14)

Reduce cognitive load. Each split is mechanical (move + re-export).

Priority order: 10 (tests to file), 12 (write.rs), 9 (supervisor), 13
(store.rs), 14 (export.rs), 11 (tcp.rs trait), 8 (tcp_bridge helpers).

### Phase 3 — Boundaries and ownership (items 15–19)

Requires design decisions. Item 11/17/18 form a cluster — the `TunnelSender`
trait unblocks all three.

### Phase 4 — Ad-hoc patterns (items 20–27)

Mixed effort. Items 20 (wire_value macro) and 23 (frame builder) have the
highest payoff. Items 24–25 are C++ and independent.

### Phase 5 — Structural (items 28–30)

Low urgency, design-level changes. Do when touching those areas.
