# remote-exec-mcp code audit — round 4

Fourth-pass audit focused on implementation quality: duplication, unclear boundaries, spaghetti, and ad-hoc code. Functional bugs are mentioned lightly where relevant. References use `file:line`.

## A. God translation units and headers

### 1. `port_tunnel_transport.cpp` — three responsibilities, 931 LOC

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp` owns:
- thread-spawn helpers `spawn_tcp_read_thread` / `spawn_tcp_write_thread` / `spawn_udp_read_thread` (72–226)
- `PortTunnelSender` write-queue class (357–613)
- `PortTunnelConnection` frame-read loop and protocol state machine (297–931)
- `TransportOwnedStreams` map (635–714)

Fix: split into `port_tunnel_sender.cpp`, `port_tunnel_spawn.cpp`, `port_tunnel_streams.cpp`, and a leaner `port_tunnel_transport.cpp` that keeps only the `PortTunnelConnection` dispatch.

### 2. `port_tunnel_internal.h` — 476-LOC kitchen-sink header included by five TUs

Included by `port_tunnel_transport.cpp`, `port_tunnel_session.cpp`, `port_tunnel_tcp.cpp`, `port_tunnel_udp.cpp`, `port_tunnel_error.cpp`. Declares `PortTunnelService`, `PortTunnelConnection`, `PortTunnelSender`, `TransportOwnedStreams`, `TunnelTcpStream`, `TunnelUdpSocket`, `RetainedTcpListener`, `PortTunnelSession`, plus the spawn helpers. Any change to any of these types rebuilds all five TUs.

Fix: split into `port_tunnel_service.h`, `port_tunnel_connection.h`, `port_tunnel_streams.h`.

### 3. Triplicated `spawn_*_thread` pattern

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:72-124,126-167,185-226` — `spawn_tcp_read_thread`, `spawn_tcp_write_thread`, `spawn_udp_read_thread` share the exact same structure: acquire worker, `#ifdef _WIN32` allocate context + `begin_win32_thread`, `#else` `std::thread([...]).detach()`, catch, release on failure. Six `#ifdef _WIN32` blocks total.

Fix: one template helper `spawn_worker_thread(service, std::function<void()>)` collapses all three.

### 4. `session_store.cpp` mixes output rendering with session lifecycle

`crates/remote-exec-daemon-cpp/src/session_store.cpp:60-160` — `floor_char_boundary`, `ceil_char_boundary`, `render_output`, `truncation_marker`: UTF-8 text processing with no dependency on `LiveSession` or `SessionStore`. The `build_response` JSON builder at 170–192 is another standalone concern.

Fix: extract `output_renderer.cpp` (or `text_utils.cpp`) and a `session_response_builder.cpp`.

### 5. `session_pump.h` exposes locked helpers publicly and creates a header cycle

`crates/remote-exec-daemon-cpp/include/session_pump.h` declares `mark_session_exit_locked`, `take_session_output_locked`, `drain_exited_session_output_locked`, `finish_session_output_locked` — all require the caller to hold `session->mutex_`. They are pump↔store implementation detail. `session_pump.h` also includes `session_store.h` (for `LiveSession`) while `session_store.cpp` includes `session_pump.h`, so the public header dependency runs both ways.

Fix: move `LiveSession` to `include/live_session.h`; relocate the `*_locked` helpers to `src/session_pump_internal.h` so they are not shipped.

## B. Duplicated code

### 6. `archive_error_to_transfer_error` / `internal_transfer_error` byte-for-byte in two files

`crates/remote-exec-host/src/transfer/archive/export.rs:533-542` and `import.rs:500-509`. Identical.

Fix: lift into `transfer/archive/mod.rs` or a new `archive/error.rs`.

### 7. Three copy-pasted `normalize_*_error` functions in broker tools

`crates/remote-exec-broker/src/tools/image.rs:73`, `tools/transfer/operations.rs:293`, `tools/transfer/endpoints.rs:261`:

```rust
DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
other => other.into(),
```

Fix: `DaemonClientError::into_anyhow_rpc_message()` method (or a single free function in `daemon_client.rs`).

### 8. Four `local_policy` / `host_policy` holdouts

`crates/remote-exec-broker/src/local_transfer.rs:153`, `tools/transfer/endpoints.rs:311`, `startup.rs:209`, `tools/exec.rs:443` — each re-writes `if cfg!(windows) { windows_path_policy() } else { linux_path_policy() }`.

Fix: single `proto::path::host_policy()`.

### 9. `"local"` magic string scattered across four broker files

`crates/remote-exec-broker/src/tools/transfer/endpoints.rs:321`, `state.rs:29`, `startup.rs:80,83`, `local_port_backend.rs:33`. A typo in any silently breaks routing.

Fix: `pub const LOCAL_TARGET_NAME: &str = "local";` in `state.rs`; import everywhere.

### 10. `validate_exec_start_response` and `validate_exec_write_response` identical

`crates/remote-exec-broker/src/tools/exec.rs:405-425` — same body.

Fix: collapse to one `validate_exec_response`.

### 11. `set_transfer_target_context` and `set_forward_ports_target_context` structurally duplicated

`crates/remote-exec-broker/src/tools/transfer.rs:113` and `tools/port_forward.rs:32` — collect names, dedup, sort, join, call `set_current_target`. Only the name-extraction differs.

Fix: `set_multi_target_context(&[&str])` helper in `request_context`.

### 12. `ScriptedTunnelIo` / `ScriptedTunnelState` duplicated across TCP/UDP tests

`crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:889-1007` and `udp_bridge.rs:302-412` — byte-for-byte identical harness (AsyncRead/AsyncWrite impls, `fail_writes`, `push_read_frame`, `wait_for_written_frame`, `pop_matching_written_frame`). Same for `wait_until_send_fails`, `filter_one`, `test_record`.

Fix: extract to `port_forward/test_support.rs` (cfg-gated).

### 13. Six-function ladder in `host::port_forward::tcp` for transport vs session contexts

`crates/remote-exec-host/src/port_forward/tcp.rs:537-571` — `cleanup_transport_tcp_stream`, `cancel_transport_tcp_stream`, `clear_transport_tcp_cancel` + `_session_` triplet. Bodies identical modulo the receiver (`tunnel.tcp_streams` vs `attachment.tcp_streams`).

Fix: `TcpStreamMap<'_>` newtype wrapping `&Mutex<HashMap<u32, TcpStreamEntry>>` with `cleanup` / `cancel` / `clear_cancel` methods; both `TunnelState` and `AttachmentState` expose it.

### 14. Broker port-forward handshake duplicated in `open_listen_session` and `open_data_tunnel`

`crates/remote-exec-broker/src/port_forward/supervisor.rs:596-650` and `:670-722` — both send a `TunnelOpen` frame, wait with the same timeout, match on `TunnelReady`/`Error`. Differ only in role/meta fields.

Fix: `open_tunnel_with_role(side, forward_id, role, generation, resume_session_id, max_queued_bytes) -> anyhow::Result<(Arc<PortTunnel>, TunnelReadyMeta)>`.

### 15. `EndpointMeta` / `TcpAcceptMeta` / `UdpDatagramMeta` defined twice

`crates/remote-exec-broker/src/port_forward/tunnel.rs:375-387` and `crates/remote-exec-host/src/port_forward/types.rs:67-85` — structurally identical wire-format structs. The broker's `TcpAcceptMeta` omits `peer`; `EndpointMeta` and `UdpDatagramMeta` are identical.

Fix: move canonical copies to `remote-exec-proto`; import from both sides.

### 16. `encode_tunnel_meta` / `decode_tunnel_meta` (broker) vs `encode_frame_meta` / `decode_frame_meta` (host)

`crates/remote-exec-broker/src/port_forward/tunnel.rs:389-395` and `crates/remote-exec-host/src/port_forward/codec.rs:10-26` — same `serde_json::to_vec` / `from_slice` wrappers, different names, different error types.

Fix: one proto helper; each side wraps the error in one line.

### 17. `TunnelRole` defined twice

`crates/remote-exec-proto/src/port_tunnel.rs:84` (wire) and `crates/remote-exec-broker/src/port_forward/events.rs:25` (loop-control) — structurally identical.

Fix: use the proto one in `ForwardLoopControl::RecoverTunnel`; if the broker-local sense really needs a distinct name, rename to `ForwardSide`.

### 18. `data_frame_charge` (broker) vs `queued_frame_charge` (host) express the same intent

`crates/remote-exec-broker/src/port_forward/tunnel.rs:333-341` charges when `stream_id != 0`; `crates/remote-exec-host/src/port_forward/mod.rs:53-61` charges when `!data.is_empty()`. Both are trying to say "is this a data-plane frame?".

Fix: single `is_data_plane_frame(&Frame) -> bool` in proto; document any intentional asymmetry.

### 19. Duplicated `TunnelReady` JSON construction (C++)

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:877-886` and `:899-907` — `listen` and `connect` branches of `tunnel_open` each build `TunnelReady` meta inline; the `limits` sub-object is identical.

Fix: `make_tunnel_ready_meta(generation, session_id_opt, limits)`.

### 20. `write_ca_pair` / `write_broker_pair` / `write_daemon_pair` near-identical

`crates/remote-exec-pki/src/write.rs:18-56` — all three build a `KeyPairPaths`, call `write_pair`, return. `write_dev_init_bundle` at :58 reconstructs `KeyPairPaths` inline, spelling filenames a second time.

Fix: single `write_named_pair(name, pair, out_dir, force)`.

### 21. `logging.rs` duplicated across three crates

`crates/remote-exec-host/src/logging.rs`, `broker/src/logging.rs`, `daemon/src/logging.rs` — `preview_text` is byte-identical in host and broker; `init_logging` bodies in broker and daemon differ only in the `DEFAULT_FILTER` literal.

Fix: a small `remote-exec-util` crate, or put shared helpers in proto (`preview_text` is a display utility; `init_logging` takes `default_filter: &str`).

## C. Unclear or leaky boundaries

### 22. `broker::lib.rs` over-exports internals as `pub`

`crates/remote-exec-broker/src/lib.rs:2-16` marks `client`, `config`, `daemon_client`, `local_backend`, `local_transfer`, `logging`, `mcp_server`, `port_forward`, `session_store`, `tools` all `pub`. Intended public surface is `build_state`, `run`, `BrokerState`, `CachedDaemonInfo`, `TargetHandle`, `install_crypto_provider`.

Fix: downgrade internals to `pub(crate)`; re-export only what the `remote_exec` binary needs.

### 23. `host::ids` typed IDs are erased at every call site

`crates/remote-exec-host/src/ids.rs` returns `ExecSessionId`, but callers (`handlers.rs:266` and `exec/store.rs` throughout) immediately `.into_string()`. The wrapper types provide no compile-time protection — callers are free to pass a `forward_id` where a `session_id` is expected. The current middle state pays the type-definition cost for none of the benefit.

Fix: either plumb typed IDs through `SessionStore` (change the map key to `ExecSessionId`), or drop the wrappers and use `new_*` free functions returning `String`.

### 24. `WarningCode` lives in the wrong submodule

`crates/remote-exec-proto/src/rpc/exec.rs:214` — the enum contains `TransferSkippedUnsupportedEntry` and `TransferSkippedSymlink`, both consumed only by `rpc/transfer.rs` via `use super::WarningCode`. The exec module re-exports it through `rpc.rs:12`.

Fix: split into `ExecWarningCode` and `TransferWarningCode` per domain, or promote `WarningCode` to `rpc/mod.rs`.

### 25. `proto::port_tunnel.rs` holds codec and meta in one file

`crates/remote-exec-proto/src/port_tunnel.rs:6-480` — protocol constants (6–12), `FrameType` + codec (14–80), meta DTOs (82–175), `Frame` (177), async read/write (185–480). Two coherent halves in one 480-LOC file.

Fix: split into `port_tunnel/codec.rs` and `port_tunnel/meta.rs`.

### 26. `mcp_server::write_test_bound_addr_file` is test infra on the production path

`crates/remote-exec-broker/src/mcp_server.rs:358` reads `REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE` from the environment and writes the bound address to it — unconditionally, in production serve paths.

Fix: `#[cfg(test)]`-gate, or move into a harness that wraps `serve_streamable_http`.

### 27. `StreamIdAllocator::set_next_for_test` missing `#[cfg(test)]`

`crates/remote-exec-broker/src/port_forward/generation.rs:42` — only used from a `#[cfg(test)]` block in `lib.rs:30`, but the method itself is compiled into production.

Fix: `#[cfg(test)]` on the method declaration.

### 28. `daemon/config/mod.rs::normalize_configured_workdir` is a pass-through

`crates/remote-exec-daemon/src/config/mod.rs:202`:

```rust
pub fn normalize_configured_workdir(path: &Path, windows_posix_root: Option<&Path>) -> PathBuf {
    remote_exec_host::config::normalize_configured_workdir(path, windows_posix_root)
}
```

Zero added behavior.

Fix: `pub use`, or delete and import from `remote_exec_host::config` directly.

### 29. `daemon/config/mod.rs::prepare_runtime_fields` is an empty stub

`crates/remote-exec-daemon/src/config/mod.rs:159` — `pub fn prepare_runtime_fields(&mut self) {}` called between `normalize_paths` and `validate`. Empty body; implies a contract that does not exist.

Fix: remove, or document as a reserved hook.

### 30. `EmbeddedDaemonConfig` adds no behavior around `EmbeddedHostConfig`

`crates/remote-exec-daemon/src/config/mod.rs:83,128` — thin wrapper whose only purpose is to hold `EmbeddedHostConfig` and produce `DaemonConfig`. The `From<EmbeddedHostConfig> for DaemonConfig` impl at :128 already does this.

Fix: remove the wrapper; keep only the `From` impl.

### 31. Inline `serde_json::json!({...})` test frames in `host/port_forward/mod.rs`

`crates/remote-exec-host/src/port_forward/mod.rs` — roughly 25 test-frame JSON literals at lines 122, 145, 191, 272, 297, 344, 398, 477, 507, 611, 635, 649, 676, 699, 714, 738, 753, 778, 794, 856, 1144, 1295, 1322. Every test rebuilds `EndpointMeta` / `TunnelOpenMeta` as raw JSON instead of using the typed structs + `encode_frame_meta`.

Fix: test helpers `endpoint_frame`, `tunnel_open_frame`, etc.; replace inline literals.

## D. Struct/flag sprawl

### 32. `ForwardRuntime` is a 12-field bag

`crates/remote-exec-broker/src/port_forward/supervisor.rs:66-82` — carries six limit scalars (`max_active_tcp_streams_per_forward`, `max_pending_tcp_bytes_per_stream`, `max_pending_tcp_bytes_per_forward`, `max_udp_peers_per_forward`, `max_tunnel_queued_bytes`, `max_reconnecting_forwards`) alongside identity/routing fields. Every test constructs all twelve inline (`tcp_bridge.rs:1122-1138`, `udp_bridge.rs:743-759`).

Fix: `ForwardLimits` sub-struct. Unlocks #33.

### 33. `ForwardRuntimeParts` is a redundant intermediate

`crates/remote-exec-broker/src/port_forward/supervisor.rs:84-95` — near-duplicate of `ForwardRuntime` with the same fields minus the limit scalars. Exists solely to feed `ForwardRuntime::new`.

Fix: delete after #32.

### 34. `close_tcp_pair_after_connect_error` / `close_tcp_pair_after_connect_pressure` near-identical

`crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:697-737` — both remove from both maps, call `release_pending_budget`, `release_active_tcp_stream`, close listen-side. The pressure variant additionally closes the connect side and calls `record_dropped_stream`.

Fix: `close_tcp_pair(runtime, state, connect_stream_id, CloseReason)` with an enum reason.

### 35. Pair teardown repeated manually at 10 call sites

`crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:340+343, 364+365, 505+510, 533+548, 706+708, 730+735, 760+764+787` — `release_pending_budget` + `release_active_tcp_stream` always called in succession; no helper.

Fix: `remove_stream_entry(state, budget, connect_stream_id) -> Option<TcpConnectStream>`.

### 36. `ListenSessionControl.generation` always `1`

`crates/remote-exec-broker/src/port_forward/supervisor.rs:309,511` — hardcoded in both `new_for_test` and `build_opened_forward`; never incremented. Passed to `open_listen_session` on resume.

Fix: either implement generation rotation on reconnect, or drop the field and comment the two call sites as `// TODO: generation rotation`.

## E. Ad-hoc code smells

### 37. Ad-hoc JSON field access in C++ HTTP route handlers

`crates/remote-exec-daemon-cpp/src/server_route_exec.cpp:105,120` call `body.at("cmd").get<std::string>()` twice; `:158,163` call `body.at("daemon_session_id").get<std::string>()` twice. Every handler re-parses the request body inline; every handler does its own exception mapping.

Fix: typed `ExecStartRequest` / `ExecWriteRequest` structs parsed once at handler entry, analogous to the existing `TunnelOpenMetadata` parser. Centralizes the `Json::exception` → wire-error mapping.

### 38. `tunnel_close` bypasses the typed metadata parser (C++)

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:918-919` — the only frame handler still doing `Json::parse(frame.meta)` + `meta.at("generation").get<std::uint64_t>()` inline. Every other handler went through `parse_tunnel_open_metadata` after `fix: type tunnel open metadata errors`.

Fix: add `parse_tunnel_close_metadata`, or reuse the generation-only subset of `TunnelOpenMetadata`.

### 39. Magic number defaults hiding in C++ config loader

`crates/remote-exec-daemon-cpp/src/config.cpp:486-491,501` — `max_request_header_bytes` (64 KB), `max_request_body_bytes` (512 MB), `max_open_sessions` (64UL) appear as bare literals in `load_config`, not alongside the `DEFAULT_PORT_FORWARD_*` named constants in `include/config.h:52-60`.

Fix: promote each to a named `static const` in `config.h`.

### 40. `local_port_backend.rs` hardcodes a separate `EmbeddedHostConfig`

`crates/remote-exec-broker/src/local_port_backend.rs:32-49` constructs `EmbeddedHostConfig` with `allow_login_shell: false`, `pty: PtyMode::None`, etc., inline. This is a second "local" host config parallel to the one in `config.rs:241`. If port-forward-only local ever needs distinct limits, there is no config path.

Fix: re-use `LocalTargetConfig::embedded_host_config`, or accept a minimal config struct.

### 41. `transfer_files` rebuilds `TransferExecutionOptions` twice per source case

`crates/remote-exec-broker/src/tools/transfer.rs:67-88` — the `match sources.as_slice()` single vs multi arms are identical except for which function they call.

Fix: build `options` once before the match.

### 42. `daemon_client::decode_rpc_error` / `decode_rpc_error_strict` asymmetry

`crates/remote-exec-broker/src/daemon_client.rs:610,621` — strict propagates body-read failures as transport errors; the other swallows body-read errors into the message string. Naming is opaque; the rationale (port-tunnel vs RPC) is undocumented.

Fix: collapse to one function with a `propagate_body_error: bool` parameter, or add a file-level comment explaining the split.

### 43. `list_targets` missing `elapsed_ms` in "broker tool completed" log

`crates/remote-exec-broker/src/tools/targets.rs:39-44` — every other tool emits `elapsed_ms` on completion; `list_targets` does not. It also never calls `request_context::set_current_target` (correctly — no single target), but the intent is undocumented.

Fix: add `elapsed_ms` + a short comment about the missing target context.

### 44. `write_stdin` double-wraps errors

`crates/remote-exec-broker/src/tools/exec.rs:140` — `anyhow::anyhow!("write_stdin failed: {err}")` discards the original error chain. All other tool handlers propagate with `?`. `format_tool_error` will render a double-wrapped string.

Fix: `.map_err(|err| { tracing::warn!(...); err })` and propagate directly.

### 45. `command_tool_result_for_logging` round-trips structured content through JSON

`crates/remote-exec-broker/src/tools/exec.rs:379-381` — `write_stdin` completion log parses `serde_json::Value` back into `CommandToolResult` to read two fields. Signals that `write_stdin_inner` should return a richer type directly.

Fix: restructure the return value of `write_stdin_inner` so logging fields are available before serialization.

### 46. Redundant `startup::build_state` re-validation

`crates/remote-exec-broker/src/startup.rs:34-35` unconditionally calls `config.normalize_paths()` + `config.validate()` that `BrokerConfig::load` (at `config.rs:308-316`) already ran. Every caller pays the cost twice, including a filesystem stat in `validate_existing_directory`.

Fix: have `build_state` take a `ValidatedBrokerConfig` newtype, or have `load` return raw and leave validation to `build_state`.

### 47. `sources.mk` manually re-expands base source lists

`crates/remote-exec-daemon-cpp/mk/sources.mk:117-145,173-199` — `HOST_SERVER_STREAMING_SRCS` and `HOST_SERVER_RUNTIME_SRCS` / `SERVER_RUNTIME_TEST_SUPPORT_SRCS` each re-list `session_store.cpp`, `session_pump.cpp`, `platform.cpp`, `basic_mutex.cpp`, `logging.cpp`, `config.cpp`, `text_utils.cpp`, `server_request_utils.cpp`, `server_transport.cpp`, `$(TRANSFER_SRCS)`, `$(POLICY_SRCS)`, `$(RPC_FAILURE_SRCS)`, `$(PORT_FORWARD_SRCS)`, `$(BASE64_SRCS)`. Each is essentially `$(BASE_SRCS)` minus `main.cpp` minus the process-session file.

Fix: a `BASE_SRCS_NO_MAIN` variable.

### 48. `mk/windows-native.mk` has no test targets

`crates/remote-exec-daemon-cpp/mk/windows-native.mk` defines only the production binary. The `WINDOWS_CAPABLE_*` test groups in `sources.mk` are consumed only by `windows-xp.mk`. If the native Windows build is ever tested the make target does not exist.

Fix: add a `windows-native-test` target parallel to the XP one.

## F. Minor

### 49. `exec/session/windows.rs::TerminalOutputState` / `TerminalOutputPerformer` are PTY-generic but live in the Windows file

`crates/remote-exec-host/src/exec/session/windows.rs:43,78` — VTE parsing for CSI device-status-report responses. Pure PTY output filter, not Windows-specific.

Fix: move to `session/mod.rs` or `session/pty_filter.rs`.

### 50. `exec/shell/unix.rs` carries a dead `_windows_posix_root` parameter

`crates/remote-exec-host/src/exec/shell/unix.rs:37` — prefixed-underscore parameter never read. Windows-only signal leaking into Unix.

Fix: remove.

### 51. `write_text_file` double-checks existence

`crates/remote-exec-pki/src/write.rs:211` — `validate_output_paths` already checked existence; the guard is repeated inside `write_text_file`.

Fix: rely on the upfront validation.

### 52. `pki/write.rs` inotify FFI inside a test module

`crates/remote-exec-pki/src/write.rs:759-864` — 106 lines of hand-rolled inotify FFI live inside a `#[cfg(test)]` module in the library file, inflating the reported size to 865 LOC. The production code split in the rest of the file is actually clean.

Fix: move to `tests/support/inotify.rs` (or a `#[cfg(test)]` helper crate).

## Staged plan

### Phase E1 — dedup that unlocks further simplification

Items 6–21. These are all near-identical code blocks; removing them stops masking other smells and cuts rebuild time. Start with #15 and #16 because they move real types into proto, which makes later cross-crate work easier.

### Phase E2 — split god modules and headers

Items 1–5 (C++ side), then #25 (`proto::port_tunnel.rs`). Each shortens compile edges and cleans up review surface. Tackle the C++ header cycle (#5) alongside item #2 so `port_tunnel_internal.h` doesn't become a second kitchen-sink split.

### Phase E3 — fix unclear boundaries and lifecycle wrappers

Items 22–30. `broker::lib.rs` (#22) and the typed-ID erasure (#23) are the two with the most ongoing cost; the rest are housekeeping.

### Phase E4 — struct/flag sprawl and ad-hoc handlers

Items 32–46. The `ForwardRuntime` / `ForwardRuntimeParts` collapse (#32–#33) is the biggest single readability win; the C++ handler-body refactor (#37, #38) is a mechanical pass that removes a lot of `try`/`catch` boilerplate.

### Phase E5 — smaller cleanups

Items 27, 31, 39, 41, 42, 43, 47–52. Bundle as a single PR; each is under 30 lines.

The two quickest wins are #9 (one const for `"local"`) and #15 (move three wire structs to proto). Both remove real maintenance hazards for under 40 lines of change each.
