# remote-exec-mcp code audit — round 3

Third-pass audit after another ~45 remediation commits landed. Round-2 fixes were spot-checked; new issues focus on areas prior rounds under-covered: C++ fork/exec paths, exec/PTY lifecycle, observability, tests, and CI. References use `file:line`.

Non-atomic multi-file patch apply is intentional product behavior and is excluded from this report.

## Round-2 fix verification

| Item | Status | Notes |
|---|---|---|
| UTF-8 carry buffer in pipe reader | Landed | `host/src/exec/session/spawn.rs:108-188`, tests cover split multibyte, invalid sequences, unfinished sequences |
| Archive size cap (streaming copy) | Landed | `host/src/transfer/archive/import.rs:403-447`. Residual note: cap trusts tar header `size` field; a lying header skips the check |
| Lock-across-await in supervisor | Landed | `broker/src/port_forward/supervisor.rs:766-773` |
| Tunnel-mode TOCTOU | Landed | `host/src/port_forward/tunnel.rs:192,268`, single-shot `claim_tunnel_mode` |
| `schedule_session_expiry` cancellation | Landed | `host/src/port_forward/session.rs:259-274`, handle aborted on reattach + re-check |
| Reliable heartbeat ACK delivery | Landed | `host/src/port_forward/tunnel.rs:145-155`, bounded `send()` |
| TCP budget release on cleanup failure | Landed | `broker/src/port_forward/tcp_bridge.rs:523-553`, first-error-deferred pattern |
| C++ `catch(...)` in `PortTunnelConnection::run` | Landed | `daemon-cpp/src/port_tunnel_transport.cpp:566` |
| C++ `u64` → `size_t` range check | Landed | `ensure_u64_fits_size_t` at `transfer_ops_import.cpp:245`, `transfer_ops_tar.cpp:396,411` |
| C++ symlink target validation | Landed | `transfer_ops_fs.cpp:146-150` |
| C++ path-utils consolidation | Landed | `platform.cpp` no longer defines `join_path` |
| C++ HTTP idle timeout in config | Landed | `include/config.h:42,59` |

Residual observation on `host/src/port_forward/tcp.rs:520-534`: `TcpEof` / `Shutdown` still use `try_send`. The treatment of queue-full is now "intentional stream cancel," which is defensible, but worth confirming the downstream remote sees EOF via the cancel path rather than a silently dropped shutdown.

## A. Correctness and security bugs (new)

### 1. Path traversal: `..` components not rejected on C++ import

`crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp:49-100` (`validate_relative_archive_path`) strips leading `./` and rejects absolute paths but never checks component-by-component for `..`. A tar entry like `foo/../../../etc/shadow` passes validation. The Rust side rejects this via `path::normalize_relative_archive_path`; the C++ side does not mirror it.

Fix: split on `/`, reject any part equal to `..`, reject any part equal to `.`, reject empty parts between separators.

### 2. `setenv` in child after `fork` — not async-signal-safe

`crates/remote-exec-daemon-cpp/src/process_session_posix.cpp:252-255` calls `setenv("LC_ALL", ...)` / `setenv("LANG", ...)` in the child between `fork` and `execvp`. `setenv` may call `malloc`/`realloc`. If another thread held the malloc lock at the moment of `fork`, the child deadlocks — POSIX only guarantees async-signal-safe calls after multi-threaded `fork`.

Fix: build an `envp` array in the parent and use `execve`.

### 3. `pipe()` without `O_CLOEXEC` — grandchild fd leak

`crates/remote-exec-daemon-cpp/src/process_session_posix.cpp:86` uses plain `pipe(fds)`. The child closes its copies manually, but if the child forks again before those closes run (e.g., a shell-script child that forks), the grandchild inherits the pipe ends. The parent's `read` may never see EOF after the original child exits.

Fix: `pipe2(fds, O_CLOEXEC)` on Linux; `fcntl(fd, F_SETFD, FD_CLOEXEC)` immediately after `pipe()` on portable paths.

### 4. No SIGTERM grace period before SIGKILL on Rust exec

`crates/remote-exec-host/src/exec/child.rs:40-57` — `terminate()` calls `std::process::Child::kill()` directly (SIGKILL on Unix). No SIGTERM + grace escalation and no process-group kill. If the child was a shell running a pipeline, its descendants are orphaned. The C++ side at `daemon-cpp/src/process_session_posix.cpp:396-410` already implements SIGTERM→sleep→SIGKILL on the process group; the Rust side is more violent and leaks subprocesses.

Fix: set a new process group on spawn (`setpgid` / `CREATE_NEW_PROCESS_GROUP`) and mirror the grace-period escalation.

### 5. Narrowing `size_t` → `int` on every socket call (C++)

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:277` and `server_transport.cpp:414`:

```cpp
::recv(fd, buf + offset, static_cast<int>(size - offset), 0);
```

On POSIX with a buffer > 2 GB the cast wraps to a negative `int`; `recv` / `send` fail with `EINVAL` or — worse on some platforms — read garbage.

Fix: `std::min(size - offset, static_cast<size_t>(INT_MAX))` before the cast, with an outer loop over the whole buffer.

### 6. SIGCHLD not handled → zombie accumulation (C++)

`crates/remote-exec-daemon-cpp/src/process_session_posix.cpp` — no `SIGCHLD` handler, no `SA_NOCLDWAIT`. `waitpid` is called only when a session is actively polled. If the HTTP connection drops before the session is reaped, the child becomes a zombie until the store's cleanup runs. Under high churn zombies accumulate.

Fix: install a reaping handler or set `SA_NOCLDWAIT` on sessions that don't need exit-status observation.

### 7. `LiveSession` has no `Drop` — child-process leak on error paths

`crates/remote-exec-host/src/exec/live.rs` — `LiveSession` owns a spawned child, but if `exec_start_local` errors after spawn (sandbox check fails, store insert races), the `LiveSession` is dropped without `terminate()`. The child runs forever, untracked.

Fix: `impl Drop for LiveSession` that aborts background tasks and signals the child; or a scope-guard `Defer` that only disarms on successful store insert.

### 8. `exec_write` has no timeout on session lock

`crates/remote-exec-host/src/exec/handlers.rs:82-91` — `state.sessions.lock(&daemon_session_id).await` waits indefinitely. If another task holds the lock while blocked on a slow PTY writer, the RPC hangs.

Fix: `tokio::time::timeout(cfg.exec_rpc_timeout, lock_fut)` with a typed RPC-timeout error.

### 9. PTY resize never propagates

`crates/remote-exec-host/src/exec/live.rs` has no `resize` method; `PtySession.master: Box<dyn MasterPty>` has one but nothing calls it. No RPC verb in `proto::rpc::exec` carries a resize event, so interactive PTY sessions are stuck at `default_pty_size()` for their lifetime. Visible as line-wrap and cursor corruption in `vim` / `less`.

Fix: add an `ExecResize` RPC or piggyback on `ExecWriteRequest`.

### 10. Broker has no end-to-end exec timeout

`crates/remote-exec-broker/src/tools/exec.rs:47-60` — `forward_exec_start` has no `tokio::time::timeout`. A hung daemon PTY read blocks the MCP tool call forever. The daemon's own `yield_time_ms` bounds its wait, but transport-level hangs are not bounded.

Fix: route every tool path through the broker's daemon-RPC timeout (the commit `fix: bound broker daemon rpc calls` added one — verify every exec call uses it, not just the generic `post()`).

### 11. `HttpAuthConfig` derives `Debug` with raw token

`crates/remote-exec-proto/src/auth.rs:4` — `#[derive(Debug, Clone, Deserialize, JsonSchema)]` on `HttpAuthConfig { bearer_token: String }`. Any future `tracing::debug!("{:?}", config)` or stray `dbg!()` prints the token. Both `BrokerConfig` and `DaemonConfig` propagate `Debug` through this field.

Fix: manual `Debug` that prints `bearer_token: <redacted>`; ideally introduce a `Secret<T>` newtype shared across the workspace.

### 12. Transient transcript tail peak above limit

`crates/remote-exec-host/src/exec/transcript.rs:29-33` extends `tail` with the full incoming chunk before draining. A single large push spikes the tail to `tail_limit + chunk_size`. Not a leak, but the "512 KB cap" is not a hard cap.

Fix: drain proactively before extend if `chunk.len() >= tail_limit`.

### 13. `send_all_bytes` called while holding `writer_mutex_`

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:343-353` — `send_frame` acquires `writer_mutex_` then calls `send_all_bytes`, which loops on `send()`. If the kernel send buffer is full the mutex is held for the entire duration of the blocking send. Any other thread trying to `send_frame` (e.g., UDP datagram sender) stalls. This is the intended serialization but means one slow TCP peer stalls all outbound tunnel frames.

Fix: non-blocking I/O with a dedicated writer task, or at minimum document the latency hazard and bound the per-call send with a timeout.

### 14. `tunnel_open` JSON fields parsed without typed error mapping

`crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:636-638` calls `meta.at("role").get<std::string>()` and peers with no per-field try/catch. `nlohmann::json` throws `json::type_error` / `json::out_of_range`. The outer `catch (const std::exception&)` maps these to `"internal_error"` instead of `"invalid_port_tunnel"`, making wire-level diagnosis harder.

Fix: wrap the parse block and map failures to `PortForwardError(400, "invalid_port_tunnel", ...)`.

### 15. Stub `validate_relative_archive_path` counterparts and `../etc` defense on both sides

Related to #1: the Rust side's relative-path validator (`host::transfer::archive::path`) has unit tests for `..`; the C++ side has no equivalent test — so a regression in the C++ validator is invisible. See also test-side item #27 for mirror-fixture divergence.

## B. Observability and operability

### 16. Daemon has no SIGTERM handler

`crates/remote-exec-daemon/src/main.rs` calls `run_until(config, pending::<()>())`. The shutdown future is `pending()`, so the daemon never self-terminates on SIGTERM — the graceful-drain logic in `http_serve.rs` never runs, and the process is hard-killed by the OS. The broker (`mcp_server.rs:270-295`) already has the handler.

Fix: mirror `wait_for_shutdown_signal` in `daemon/src/main.rs`.

### 17. `error!` level for routine 4xx

`crates/remote-exec-daemon/src/http/request_log.rs:16` emits `error!` for all non-2xx responses. Auth failures, bad requests, and sandbox denials swamp the error-level stream and mask real server faults.

Fix: `warn!` for 4xx, `error!` only for 5xx.

### 18. `info!` on every `write_stdin` poll

- `crates/remote-exec-broker/src/tools/exec.rs:95-101`
- `crates/remote-exec-host/src/exec/handlers.rs:77-83`

Every poll — including empty polls — emits `info!`. An interactive session produces a steady `info` stream.

Fix: gate `empty_poll = true` to `debug!`.

### 19. No request correlation ID across broker → daemon

`crates/remote-exec-daemon/src/http/request_log.rs` logs `method/path/status/elapsed_ms` but no request ID. Broker logs `daemon_instance_id` only for exec completions. No `#[tracing::instrument]` on handlers. Under concurrent load it is impossible to join a broker error line to a specific daemon log entry.

Fix: generate an `x-request-id` per broker call, log on both sides, thread through a `tracing::Span`.

### 20. Startup warnings drop `base_url`

`crates/remote-exec-broker/src/startup.rs:190-204` — `log_remote_target_unavailable` and `log_remote_target_startup_probe_timeout` log `target` and `http_auth_enabled` but not `base_url`. The success path at line 172 does log it. An operator sees "target X unavailable" with no URL or TLS chain.

Fix: add `base_url = %target_config.base_url` to both warn sites.

### 21. Patch audit trail is thin

- `crates/remote-exec-broker/src/tools/patch.rs:26-50` logs `patch_len` and `has_workdir` only.
- `crates/remote-exec-host/src/patch/mod.rs:35-39` logs the updated-paths count but not the paths themselves.
- Broker does not log `daemon_instance_id` on patch completion, unlike exec.

Exec logs a `cmd_preview` (truncated to 120 chars); patch should log the path summary and the daemon instance for the same auditability.

### 22. Zero structured metrics

No `metrics`, `prometheus`, `opentelemetry`, or `statsd` crate in the workspace. Tunnel throughput, session counts, error rates, and port-forward lifecycle are all unstructured `info!`/`warn!`. Alerting requires log scraping.

Fix: add a `metrics` crate façade and emit counters from event sites that already have `tracing::event!`.

### 23. Exit codes degenerate to 0/1

`crates/remote-exec-broker/src/bin/remote_exec.rs:742-743` returns `1` for any error. Scripting consumers cannot distinguish a remote command failure from a connection failure from a config error.

Fix: partition into 2 (usage), 3 (config), 4 (connection), 5 (tool error).

### 24. C++ ↔ Rust daemon config drift

- `port_forward_tunnel_io_timeout_ms` exists in C++ (`daemon-cpp/src/config.cpp:332-335`); absent in Rust `HostPortForwardLimits` (`host/src/config/mod.rs:67-86`).
- `max_request_header_bytes`, `max_request_body_bytes`, `max_open_sessions` exist in C++ (`daemon-cpp/src/config.cpp:479-495`); absent in Rust daemon (hyper defaults, not operator-visible).
- `port_forward_max_worker_threads` exists in C++; architecturally N/A in Rust, but example files don't call out the deliberate gap.

Fix: add Rust equivalents where architecturally meaningful; annotate deliberate gaps in `configs/daemon.example.toml`.

### 25. MCP tool errors carry no correlation ID

`crates/remote-exec-broker/src/mcp_server.rs:52-54` — `format_tool_error` returns the raw `anyhow` chain as `Content::text`. No target name, no request timestamp, no daemon instance ID. The MCP client cannot correlate the error to a broker log line.

Fix: include a correlation ID + target name in the returned error and in the corresponding log line.

## C. Tests and CI

### 26. Stub daemon hard-codes `"daemon-instance-1"` and `"daemon-session-1"`

- `crates/remote-exec-broker/tests/support/stub_daemon.rs:585-590` — `/v1/health` returns a literal string; real daemon returns `inst_<uuid>` (`host/src/ids.rs:47`).
- `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs:84` — `assert_eq!(req.daemon_session_id, "daemon-session-1")` panics inside the handler, producing a 500 rather than a typed `unknown_session` response.
- `crates/remote-exec-broker/src/tools/exec_format.rs:135` uses the same literal in a unit fixture.

The round-2 ID-prefix refactor is effectively untested: stubs accept only the old format. A drift in the real daemon format would not be caught.

Fix: stubs return fixture values from `ids::new_instance_id()`; assertion becomes a typed error mapping.

### 27. TOCTOU on ephemeral port allocation across ~15 test sites

`crates/remote-exec-broker/tests/support/certs.rs:39-44` and `tests/multi_target/support.rs:646-651` bind to `:0`, read the address, drop the listener, then hand the bare address to the server. In the gap another process or parallel test can claim the port. Expected to flake on loaded CI.

Fix: keep the listener bound and hand it through; or retry on `AddrInUse`.

### 28. Temp-dir leak per stub test

`crates/remote-exec-broker/tests/support/stub_daemon.rs:732-756` creates `std::env::temp_dir().join("remote-exec-broker-stub-port-tunnel-<uuid>")` and never deletes it.

Fix: wrap in `tempfile::TempDir` and return it alongside state.

### 29. `tokio::spawn` without handles swallows test panics

`stub_daemon.rs:497,521,633,811` and `spawners.rs:305,512,586,628` discard `JoinHandle`. A panicked task is silently dropped; the test may pass despite a broken stub. The tunnel relay at `stub_daemon.rs:811` is the highest-risk site — a silent stop makes a broken test look like a legitimate timeout.

Fix: join handles on teardown and assert no panics; or install a panic hook that aborts the test.

### 30. Sleep-as-synchronization in port-forward tests

- `crates/remote-exec-broker/tests/mcp_forward_ports.rs:183` and `tests/multi_target.rs:342,463,561` — `tokio::time::sleep(Duration::from_millis(250))` then assert state.
- C++: `daemon-cpp/tests/test_session_store.cpp:397,653` — `platform::sleep_ms(200)` / `sleep_ms(150)`.
- C++: `daemon-cpp/tests/test_server_streaming.cpp:1547` — `sleep_ms(5000)` to hold a socket buffer full.

Expected to flake on slow Windows and macOS runners. `wait_for_forward_status_timeout` already exists in the same file; these callers didn't migrate.

Fix: replace with condition-variable polling or existing wait helpers.

### 31. Negative UDP assertions with tight deadlines

`crates/remote-exec-broker/tests/mcp_forward_ports.rs:1209,1608` — `tokio::time::timeout(Duration::from_millis(100), socket.recv_from(...))` asserts no packet arrives. On a loaded runner 100 ms may not be enough for a real packet to arrive, causing a false negative assertion pass.

Fix: raise the negative-assertion window (e.g., 500 ms) and add a positive counterpart that verifies legitimate packets do arrive within the same window.

### 32. XP test binaries built but never executed in CI

`.github/workflows/ci.yml:204,221` — `check-windows-xp` builds `test_session_store-xp.exe` and `test_transfer-xp.exe` but there is no `wine` step. `mk/windows-xp.mk:72-73` defines `test-wine-session-store` / `test-wine-transfer` targets that would run them under Wine, but CI never invokes them. The XP test binaries compile with `-std=c++11` (correct), but their runtime correctness is never verified.

Fix: add a Wine step in CI, or a native Windows-XP VM job.

### 33. `no-default-features` CI job misses `remote-exec-host`

`.github/workflows/ci.yml:91-94` runs `cargo test -p remote-exec-broker --no-default-features` and `cargo test -p remote-exec-daemon --no-default-features`. `remote-exec-host` — which owns most of the exec/transfer/port-forward logic — is not tested without default features. A broken `#[cfg(feature = "...")]` gate in host is only caught by the full-feature run.

Fix: add `cargo test -p remote-exec-host --no-default-features` to the matrix.

### 34. `wait_until_ready` loops lack an outer timeout

- `stub_daemon.rs:696-709,712-730` (up to 40 × 50 ms = 2 s)
- `spawners.rs:960-978` (80 × 50 ms = 4 s)
- `multi_target/support.rs:698-712` and `:746-765` (up to 400 × 50 ms = 20 s on Windows)

No outer `tokio::time::timeout` wraps the loop. If the daemon never starts, the test hangs for the full budget before panicking, obscuring which test actually failed.

Fix: wrap each readiness loop in `tokio::time::timeout` with a clear error message naming the resource.

### 35. Stub `patch_apply` validates header only

`crates/remote-exec-broker/tests/support/stub_daemon.rs:661-673` checks `patch.starts_with("*** Begin Patch\n")` but not the `"*** End Patch"` footer that the real parser requires (`host/src/patch/parser.rs:93-96`). A broker test could send a malformed patch the stub accepts, giving false confidence.

Fix: mirror the real parser's header + footer check.

### 36. Asymmetric C++ daemon coverage on Windows

`crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs:1` is `#![cfg(unix)]`. `multi_target.rs` has no file-level `cfg(unix)` gate; only two tests at lines 735 and 8 are gated. The remaining ~15 port-forward tests in `multi_target.rs` run on Windows against the Rust daemon, which is fine — but the C++ daemon path is never exercised on Windows CI.

Fix: either build a Windows-compatible spawner for the C++ daemon or document the gap in the CI README.

## Staged plan

### Phase D1 — correctness and security (must ship first)

Items 1–5 plus 13. `..` traversal (#1) and `setenv`-after-`fork` (#2) are high-severity POSIX-side bugs; `pipe()` without `O_CLOEXEC` (#3) compounds #2 under shell workloads; SIGKILL-only on Rust (#4) leaks subprocesses; `size_t`→`int` narrowing (#5) is a real correctness hazard on large buffers; #13 is latency-at-scale on the C++ writer. These are the items an attacker or a production operator will notice first.

### Phase D2 — lifecycle and timeouts

Items 6–10, 12, 14. Zombie reaping, `LiveSession::Drop`, `exec_write` lock timeout, PTY resize, broker exec timeout, transcript tail bound, C++ tunnel-open error typing. Each closes a real runtime hole that current tests don't catch.

### Phase D3 — observability

Items 11, 15–25. Secret-redacting `Debug`, request correlation IDs, daemon SIGTERM handler, log-level discipline, patch audit trail, metrics façade, exit-code taxonomy, config drift between the C++ and Rust daemons, MCP error correlation. Bundle as an "operator experience" PR.

### Phase D4 — test reliability

Items 26–36. Stub-daemon fixture divergence, port-allocation TOCTOU, temp-dir leaks, silent panics, sleep-as-synchronization, XP-binary execution gap, no-default-features coverage, stub parser parity. These are the items that will cause intermittent CI failures and mask real regressions until addressed.

Start with D1: items #1 and #2 alone are worth a targeted PR with regression tests (crafted `..` archive; threaded `fork` stress test) because they are the ones most likely to bite someone outside the development loop.
