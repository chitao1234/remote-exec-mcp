# remote-exec-mcp code audit

Scope: all crates under `crates/` (Rust workspace plus the C++ daemon). References use `file:line`. Items are grouped by theme so duplication and cross-cutting patterns are visible, then ordered into a staged remediation plan at the end.

## Cross-cutting issues

### 1. Duplicate type definitions across the proto/public boundary

- `TransferSourceType`, `TransferSymlinkMode`, `TransferOverwrite(Mode)` declared twice: `crates/remote-exec-proto/src/public.rs:82,100,109` vs `crates/remote-exec-proto/src/rpc.rs:134,161,188`. The broker converts between them at the boundary — pure boilerplate.
- `TransferImportRequest` vs `TransferImportMetadata` at `crates/remote-exec-proto/src/rpc.rs:265–309` are identical 6-field structs with two `From` impls doing field-by-field clones. Same pattern for `TransferExport*`.
- Fix: keep one canonical definition in `rpc.rs` (or a new `proto/transfer.rs`), derive `JsonSchema`, re-export from `public.rs`. Eliminates the wire-vs-public conversion layer.

### 2. Stringly-typed error and warning codes everywhere

- `ExecWarning.code: String` (`rpc.rs:70`), `TransferWarning.code: String` (`rpc.rs:215`), `RpcErrorBody.code: String` (`rpc.rs:549`).
- Magic-string comparison in `crates/remote-exec-host/src/port_forward/error.rs:22` (`error.code == "port_tunnel_limit_exceeded"`).
- `"bad_request"` literal duplicated at `crates/remote-exec-daemon/src/rpc_error.rs:7`, `crates/remote-exec-daemon/src/port_forward.rs:94`, `crates/remote-exec-daemon/src/http/version.rs:16`. `"unauthorized"` inline at `crates/remote-exec-daemon/src/http/auth.rs:37`.
- Parallel `match` ladders in `crates/remote-exec-broker/src/daemon_client.rs:65–83` (`RpcErrorCode::wire_value` / `from_wire_value`) must stay in sync by hand.
- Fix: single `ErrorCode` enum in proto with `as_wire()` / `from_wire()`; use it in both warnings and error bodies. Replace the broker ladder with an `EnumString`/`AsRefStr` derive or a const array.

### 3. Duplicated connection-serve loops (TLS enabled vs disabled)

- `crates/remote-exec-daemon/src/tls.rs:71–143` (`serve_http_with_shutdown`) and `crates/remote-exec-daemon/src/tls_enabled.rs:62–144` (`serve_tls_with_shutdown`) differ only in the accept step. Same `JoinSet`, same shutdown watch, same per-connection `select!`.
- Similar split in `crates/remote-exec-broker/src/broker_tls_enabled.rs` / `broker_tls_disabled.rs` per `broker_tls.rs:1–5`.
- Fix: extract `serve_connections<S: AsyncRead + AsyncWrite>` that takes an already-accepted stream; both paths call it once they have a stream. Same for broker.

### 4. Scattered, inconsistent ID generation

- `uuid::Uuid::new_v4().to_string()` at `crates/remote-exec-host/src/state.rs:54` and `crates/remote-exec-host/src/exec/handlers.rs:237` (hyphenated form).
- `format!("sess_{}", uuid::Uuid::new_v4().simple())` at `crates/remote-exec-host/src/port_forward/tunnel.rs:311` (prefixed, no hyphens).
- C++ side: `"cpp-<ms>-<seq>"` at `crates/remote-exec-daemon-cpp/src/session_store.cpp:25` and `"sess_cpp_<ms>_<seq>"` at `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp:65` — the literal `"cpp"` prefix leaks implementation identity across the RPC boundary.
- Fix: `host::ids::{new_instance_id, new_session_id}` helpers on the Rust side; a proto-documented opaque ID format consumed by both Rust and C++ daemons.

### 5. `Option<T>` where invariants make it always present

- `ListTargetDaemonInfo.port_forward_protocol_version: Option<u32>` (`public.rs:66`) but `rpc.rs:26` always emits it.
- `ExecResponse.daemon_session_id: Option<String>` (`rpc.rs:57`) vs `ExecWriteRequest.daemon_session_id: String` (`rpc.rs:47`). Callers must handle `None` even when a session definitely exists.
- `last_reconnect_at: Option<String>` (`public.rs:256`) carries an ISO-8601 timestamp as a raw string; hard-coded in tests at `public.rs:396`.
- Fix: split `ExecResponse` into `ExecStartResponse` / `ExecPollResponse`; introduce a `Timestamp` newtype (or `chrono::DateTime<Utc>`); tighten `port_forward_protocol_version` to non-optional.

## Rust structural smells

### 6. `host/src/port_forward/mod.rs` is a de-facto types module disguised as a facade (1420 LOC)

- Core types (`TunnelState`, `TunnelSender`, `TcpStreamEntry`, `TcpWriterHandle`, `TunnelMode`, `TransportUdpBind`, `ErrorMeta`, `EndpointMeta`) all live here at `crates/remote-exec-host/src/port_forward/mod.rs:37–127`, while logic using them is spread across `tunnel.rs`, `tcp.rs`, `udp.rs`, `session.rs`. `mod.rs` also owns `send_forward_drop_report` and `queued_frame_charge`.
- Fix: extract to `port_forward/types.rs`; `mod.rs` becomes re-exports only.

### 7. `broker/src/port_forward/` TCP and UDP bridges are copy-paste twins

- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs:39` `run_tcp_forward` vs `udp_bridge.rs:26` `run_udp_forward`: identical except UDP increments `dropped_udp_datagrams` on the `Connect` recover arm.
- Identical 4-arm `classify_recoverable_tunnel_event` `select!` block repeated ~6 times across both files (`tcp_bridge.rs:106–195`, `udp_bridge.rs:84–245`).
- UDP's `connector_by_peer` + `peer_by_connector` maps (`udp_bridge.rs:67–69,250–251,302–303,313–314`) are always locked together but are two separate `Arc<Mutex<HashMap>>`, including a TOCTOU window in `sweep_idle_udp_connectors` at lines 319 and 329–330.
- `store.update_entry` inline closures repeated 8 times in `udp_bridge.rs:48,114,123,145,165,204`.
- `ForwardRuntimeParts` is a near-identical copy of `ForwardRuntime` in `supervisor.rs:66–118`, ~35 lines of builder noise.
- `open_protocol_forward` (`supervisor.rs:288–402`) is a 115-line function; the same `format!` context string is built twice (lines 336–339 and 345–348).
- Fix: `run_forward_loop(runtime, on_connect_recover)` generic over protocol; `handle_tunnel_side_event` helper returning `ControlFlow`; `UdpConnectorMap` struct holding both maps under one lock; `runtime.record_dropped_datagram()` / `record_dropped_stream()` helpers. Split `open_protocol_forward` into `open_listen_session_for_forward` + `build_forward_record`. Replace `ForwardRuntimeParts` with a plain constructor.

### 8. `ListenSessionControl` has two mutexes always taken together

- `crates/remote-exec-broker/src/port_forward/supervisor.rs:157–158`: `current_tunnel` and `op_lock` — `with_exclusive_operation` always takes `op_lock` first, so the ordering is implicit and fragile.
- Fix: one `Mutex<ListenSessionState>`.

### 9. `Arc<Mutex<Option<JoinHandle<()>>>>` threaded through three types

- Appears in `crates/remote-exec-broker/src/port_forward/store.rs:289`, `supervisor.rs:176,301` (`OpenedForward`, `PortForwardRecord`, `PortForwardCloseHandle`). The `Arc` exists only because both the store entry and `OpenedForward` reach for the handle.
- Fix: let the store own the handle; call sites get a close token / weak reference.

### 10. `TransferError` / `ImageError` structural copies

- `crates/remote-exec-host/src/error.rs:25–214`: both follow Kind enum + `message: String` + constructors + `code()` + `into_host_rpc_error()` with the same 500/400 branching.
- Fix: macro or generic `DomainError<K>`; at minimum a shared free function for the RPC mapping.

### 11. `expect` / `unwrap` in production paths

- `crates/remote-exec-broker/src/daemon_client.rs:612–614`: `install_crypto_provider().unwrap()` and `reqwest::Client::builder().build().unwrap()` on the MCP startup path.
- `crates/remote-exec-broker/src/main.rs:5` and `crates/remote-exec-daemon/src/main.rs:5`: `std::env::args().nth(1).expect("config path")`.
- `crates/remote-exec-daemon/src/rpc_error.rs:13`: `StatusCode::from_u16(status).expect("valid host rpc status")`.
- `crates/remote-exec-pki/src/write.rs:148,241` and `manifest.rs:51,66`: four `expect` calls in validated/manifest paths, including `"clock must be after epoch"` which is breakable under container clocks.
- Fix: propagate via `anyhow::Result` or `clap`-parsed args; for the clock case return a typed error or fall back to a sentinel.

### 12. Error-conversion call-site drift in daemon handlers

- `crates/remote-exec-daemon/src/exec/mod.rs:20,30` use `.map_err(host_rpc_error_response)` directly.
- `image.rs:18`, `transfer/mod.rs:28,45,82` use `.map_err(|err| host_rpc_error_response(err.into_host_rpc_error()))` — the extra `into_host_rpc_error()` hop is inconsistent.
- `rpc_error::bad_request` at `rpc_error.rs:7` re-enters through `exec::rpc_error`, a 3-hop chain for a trivial tuple.
- Fix: one `From<DomainError> for HostRpcError`, drop the `into_host_rpc_error()` wrapper, let `bad_request` build the tuple directly.

### 13. `anyhow` at internal boundaries loses typed errors

- `crates/remote-exec-host/src/transfer/archive/import.rs` returns `anyhow::Result` from public archive functions; `TransferError` is immediately `.into()`'d. The handler layer then must downcast or re-wrap.
- `crates/remote-exec-host/src/exec/support.rs:44` and `host/src/port_forward/error.rs:10` define two identical `rpc_error` helpers (the latter `pub(super)` to avoid a cross-module import).
- Fix: return `Result<_, TransferError>` from archive code; single `pub(crate) rpc_error` in `host::error`.

### 14. Detached `tokio::spawn` in the daemon port-forward path

- `crates/remote-exec-daemon/src/port_forward.rs:26`: fire-and-forget `tokio::spawn` for the tunnel task, not tracked in a `JoinSet` and not wired to the connection shutdown watch used elsewhere.
- Fix: register with the connection `JoinSet` or `select!` on `state.shutdown.cancelled()` inside the task.

### 15. `host_runtime_config` / `into_host_runtime_config` field-by-field twins

- `crates/remote-exec-daemon/src/config/mod.rs:131–165`: one clones, one moves.
- Fix: `impl From<DaemonConfig> for HostRuntimeConfig` and have the borrowing variant clone into it.

### 16. Three-layer heartbeat constant override

- `crates/remote-exec-host/src/port_forward/mod.rs:33–40`: `#[cfg(not(test))]` / `#[cfg(test)]` const pair, wrapped by accessors, then additional `#[cfg(debug_assertions)]` env-var overrides on top.
- Fix: a `PortTunnelTimings` struct with a `for_test()` constructor; single source of truth.

### 17. Hot-path allocation on every authenticated request

- `crates/remote-exec-daemon/src/http/auth.rs:28`: `format!("Bearer {}", bearer_token)` per request.
- Fix: pre-format once at config-validation time and store on the Arc'd config.

### 18. `BundledArchiveSource.source_path` is a `String`

- `crates/remote-exec-host/src/transfer/archive/mod.rs:36–41`: `source_path: String` alongside `source_policy: PathPolicy`; callers must remember to apply `normalize_for_system`. Adjacent `archive_path: PathBuf` is already typed.
- Fix: change to `PathBuf`, normalize at construction time.

### 19. Sandbox absoluteness check duplicated

- `crates/remote-exec-proto/src/sandbox.rs:98` (`authorize_path`) checks absoluteness via `canonicalize_for_sandbox`; `crates/remote-exec-host/src/transfer/archive/import.rs:61` explicitly calls `is_input_path_absolute` before calling `authorize_path`.
- Fix: surface a distinct `SandboxError::NotAbsolute` variant and drop the pre-check.

### 20. Daemon `exec/mod.rs` re-exports host internals publicly

- `crates/remote-exec-daemon/src/exec/mod.rs:8–11` re-exports `ensure_sandbox_access`, `resolve_input_path`, `resolve_workdir`, `session`, `store`, `transcript` — host internals — as `pub` items. Nothing in the daemon itself uses them.
- Fix: downgrade to `pub(crate)` or move the re-exports to the embedded-host integration point.

### 21. Small grab-bags and repeated literals

- `crates/remote-exec-daemon/src/logging.rs:27–33` defines `preview_text`, a string utility unrelated to logging setup.
- `crates/remote-exec-daemon/src/tls_enabled.rs:51,150`: duplicated literal `"tls config is required when transport = \"tls\""`.
- `crates/remote-exec-daemon/src/config/environment.rs:38–58`: `set_var` manually syncs `self.path` and `self.comspec` via three `eq_ignore_ascii_case` checks instead of computing them lazily.
- `crates/remote-exec-daemon/src/tls.rs:161–218`: every item in the test module is individually gated with `#[cfg(not(feature = "tls"))]`; wrap the inner module once.

## C++ daemon smells

### 22. Duplicated path utilities across translation units

- `crates/remote-exec-daemon-cpp/src/patch_engine.cpp:193–257` defines `join_path`, `parent_directory`, `make_directory_if_missing`, `create_parent_directories` in an anonymous namespace.
- `transfer_ops_fs.cpp:132–189` redefines `join_path` and `make_directory_if_missing` with identical logic.
- `path_policy.cpp:13,66` and `filesystem_sandbox.cpp:68,250` each define private `lowercase_ascii` and `comparison_key`; sandbox uses its own copy of `lexical_normalize_for_policy`.
- `patch_engine.cpp:62–109` and `patch_engine.cpp:111–179`: two near-identical path-normalization loops.
- Fix: shared `path_utils.h/.cpp`; expose `lowercase_ascii`/`comparison_key` from `path_policy.h`; unify the two normalization loops behind a flag.

### 23. RAII gaps

- `crates/remote-exec-daemon-cpp/src/session_store.cpp:348,362`: `new std::thread(...)` stored as raw pointer, `delete thread` after `join()`. Only raw-owned `std::thread*` in the codebase. Use `std::unique_ptr<std::thread>`.
- `patch_engine.cpp:273–288`: `write_text_atomic` leaks `.tmp` on rename failure; add `std::remove` before re-throw.
- Three Win32 context structs at `port_tunnel_transport.cpp:39–70` (`TcpReadContext`, `TcpWriteContext`, `UdpReadContext`) are structurally identical. Collapse behind one `spawn_worker_thread` helper.
- `session_store.cpp:699–757`: manual `pending_starts_` decrement plus a `catch (...) { release_pending_start(...); throw; }` guard — risk of double-decrement if the intervening call throws. Use a RAII guard.

### 24. Silent error swallowing

- `catch (...)` that returns `false` with no log at `port_tunnel_transport.cpp:113,151,205`, `port_tunnel_tcp.cpp:46,156`, `port_tunnel_udp.cpp:53,175`.
- Fix: log failure before returning; ideally propagate a typed error.

### 25. `truncate_path_for_header` silently truncates to 100 bytes

- `transfer_ops_tar.cpp:65–69`: redundant when callers emit a GNU LongLink header first, but silent corruption if a caller forgets. Assert/throw instead of truncating.

### 26. Session ID format leaks implementation identity

- `session_store.cpp:25`: exec IDs `"cpp-<ms>-<seq>"`. `port_tunnel_session.cpp:65`: tunnel IDs `"sess_cpp_<ms>_<seq>"`. The literal `"cpp"` crosses the RPC boundary; Rust daemon IDs do not have this marker.
- Fix: opaque format agreed in proto, identical shape across daemons.

### 27. XP test build uses `-std=gnu++17` while production uses `-std=c++11`

- `crates/remote-exec-daemon-cpp/mk/common.mk:6` sets `TEST_CXXFLAGS := -std=gnu++17`; `mk/windows-xp.mk:11` inherits this as `WINDOWS_XP_TEST_CXXFLAGS`. Tests can silently rely on C++17 features that would break the XP production build.
- Fix: align XP test `-std` with production, or add a CI job that compiles XP tests with `-std=c++11`.

### 28. bmake `Makefile` has no `link_host_test` macro

- `Makefile:138–188`: 13 near-identical link rules. GNUmakefile already has the macro; bmake doesn't. Add a `.for` loop.

### 29. Minor

- `transfer_http_codec.cpp:63–77`: two near-identical `require_one_of` overloads differing only in arity — collapse to variadic template or `std::initializer_list`.
- `process_session_posix.cpp:396–410`: `terminate()` and `terminate_descendants()` both implement the SIGTERM→sleep→SIGKILL sequence. Extract `kill_process_group(pid_t)`.

## PKI / admin smells

### 30. Private keys in unzeroized `String`

- `crates/remote-exec-pki/src/generate.rs:14–17`: `GeneratedPemPair { cert_pem: String, key_pem: String }` cloned freely (`generate.rs:58`, `write.rs:186`).
- `CertificateAuthority::pem_pair` is `pub` at `generate.rs:28–29`; `crates/remote-exec-admin/.../certs.rs:37` clones the CA key out directly.
- `lib.rs:6–13` re-exports `CertificateAuthority`, whose `issuer: Issuer<'static, KeyPair>` field is `rcgen`; callers inherit `rcgen` as a transitive dependency.
- Fix: `PrivateKeyPem(zeroize::Zeroizing<String>)`; keep `pem_pair` private with a `ca_cert_pem()` accessor and operation methods (`issue_broker_cert`, `issue_daemon_cert`) on `CertificateAuthority`. Removes the need for callers to name `rcgen` types.

### 31. Atomic-write permission handling

- `crates/remote-exec-pki/src/write.rs:207,221–235`: permissions are set on the `.tmp` file before rename, and `_mode` is silently ignored on non-Unix (underscore-prefixed parameter). Key files land with default umask on Windows.
- `write.rs:76,209`: `write_text_file` re-checks `path.exists() && !force` after `validate_output_paths` already validated it — TOCTOU plus duplicated logic.
- Fix: set permissions after rename; make the non-Unix path either an explicit error or a documented limitation; remove the redundant existence check.

### 32. `build_single_daemon_spec` fakes a `DevInitSpec` to reuse `validate()`

- `crates/remote-exec-admin/.../certs.rs:151–157` builds a `DevInitSpec` with `"unused"` strings solely to invoke `.validate()`.
- `crates/remote-exec-pki/src/spec.rs:41–84`: `DevInitSpec::validate` never checks `ca_common_name` or `broker_common_name` at all; empty or whitespace values silently produce certs TLS stacks may reject.
- Fix: `DaemonCertSpec::validate()` of its own; add non-empty/trim checks to the two missing common-name fields.

### 33. Hardcoded placeholders emitted as live config

- `crates/remote-exec-pki/src/manifest.rs:111`: `base_url = "https://{target}.example.com:9443"`.
- `manifest.rs:133`: `listen = "0.0.0.0:9443"`.
- `manifest.rs:176–196`: `sample_manifest()` hard-codes `C:\Users\chi\AppData\Local\Temp\...` — developer username leak.
- Fix: emit placeholders as comments (or omit them); use a generic or `tempdir` path in the fixture.

### 34. Duplicate CA-filename knowledge

- `crates/remote-exec-pki/src/write.rs:22–23` and `crates/remote-exec-admin/.../certs.rs:172` independently hard-code `"ca.pem"` / `"ca.key"`.
- Fix: `pub const CA_CERT_FILENAME` / `CA_KEY_FILENAME` in `pki`, referenced from both sides.

### 35. CLI flag drift between sibling commands

- `crates/remote-exec-admin/src/cli.rs:42` (`DevInitArgs`) uses `--daemon-san`; `cli.rs:109` (`IssueDaemonArgs`) uses `--san`. Same format, same parser.
- Fix: standardize on `--san`.

### 36. `x509_parser` error discarded

- `crates/remote-exec-pki/src/generate.rs:110`: `.map_err(|_| anyhow::anyhow!("parsing CA certificate DER"))` drops the underlying error.
- Fix: include `{e}` in the message.

## Staged remediation plan

### Phase 1 — high leverage, low blast radius

Focus on the proto and error surface. These are mechanical changes with broad payoff.

1. Introduce `proto::ErrorCode` enum; replace string codes in proto, broker, daemon, and host (items 2, 13).
2. Collapse duplicate transfer DTOs into one canonical set in `proto::transfer` (item 1).
3. Add `host::ids` helpers; retire ad-hoc UUID formatting; define opaque session ID format in proto and update C++ daemon (items 4, 26).
4. Replace `expect` / `unwrap` in `main.rs` entry points and `DaemonClient::new` with proper error propagation (item 11).
5. Tighten the remaining `Option<T>` fields where the invariant is always-present and split `ExecResponse` (item 5).

### Phase 2 — structural consolidation

6. Extract `host::port_forward::types` from `mod.rs` (item 6).
7. Collapse TCP/UDP bridges behind `run_forward_loop` + `handle_tunnel_side_event`; introduce `UdpConnectorMap`; unify `store.update_entry` closures as `ForwardRuntime` methods; split `open_protocol_forward` (item 7).
8. Merge `ListenSessionControl`'s two mutexes (item 8); let the store own the port-forward `JoinHandle` (item 9).
9. Unify TLS/plain connection-serve loops via generic `serve_connections` in both broker and daemon (item 3).
10. Shared C++ `path_utils.h/.cpp`; expose path-policy helpers; unify patch-engine normalization loops (item 22).

### Phase 3 — smaller cleanups and follow-ups

11. `DomainError<K>` or macro for `TransferError`/`ImageError` (item 10).
12. Daemon error-conversion alignment (item 12).
13. Track the daemon port-forward spawn in a `JoinSet` (item 14).
14. Collapse `host_runtime_config` / `into_host_runtime_config` to one `From` impl (item 15).
15. Pre-compute the `Bearer` header (item 17); `PortTunnelTimings` struct (item 16).
16. RAII fixes in C++: `unique_ptr<thread>`, `.tmp` cleanup on rename failure, log-then-return in `catch(...)` paths, align XP test `-std` flag, bmake `link_host_test` macro (items 23, 24, 27, 28).
17. PKI: zeroized key type, `CertificateAuthority` operations as methods, permission application after rename on Unix plus explicit non-Unix behavior, CA filename constants, validated common names, `--san` flag alignment, remove placeholder live config, preserve underlying parser errors (items 30–36).

Phase 1 gives the biggest payoff: it eliminates most stringly-typed plumbing, kills the largest duplicate block in `proto`, and makes failure modes visible. Phases 2 and 3 are mostly mechanical once Phase 1 lands.

