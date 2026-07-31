# remote-exec-mcp

`remote-exec-mcp` is a remote-first MCP server for running Codex-style local
system tools on configured Linux and Windows machines. Agents connect to one
broker, choose an explicit target, and use familiar tools for command execution,
stdin, patching, image reads, file transfer, and TCP/UDP forwarding.

The tool interface is influenced by
[Codex](https://github.com/openai/codex), but this repository is a separate
broker plus daemon implementation.

## Read This First

- Start with `configs/broker.example.toml` and `configs/daemon.example.toml`.
- Use `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini` for the
  standalone C++ daemon.
- Use `skills/using-remote-exec-mcp/SKILL.md` for agent-facing tool guidance.
- Treat `docs/` as historical planning and audit context unless a task
  explicitly asks for maintenance there.
- The stable external contracts are the MCP tool schemas, operator config
  files, this README, `AGENTS.md`, and the user-facing skill.

## Quick Start

Generate development certificates for Rust TLS targets:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs \
  --target builder-a
```

Start a Rust daemon:

```bash
cargo run -p remote-exec-daemon -- configs/daemon.example.toml
```

Start the broker:

```bash
cargo run -p remote-exec-broker -- configs/broker.example.toml
```

List configured targets through the CLI:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  list-targets
```

Run the C++ daemon when you need a smaller daemon or legacy Windows coverage.
It supports optional OpenSSL mutual TLS and explicit plain HTTP:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
crates/remote-exec-daemon-cpp/build/remote-exec-daemon-cpp \
  crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini
```

## Public Surface

Implemented public MCP tools:

| Tool | Purpose |
| --- | --- |
| `list_targets` | Report configured targets and cached daemon metadata. |
| `exec_command` | Start a command on one target. |
| `write_stdin` | Write to or poll a live command session. |
| `apply_patch` | Apply a Codex-style patch on one target. |
| `view_image` | Read an image from one target. |
| `transfer_files` | Transfer files or directory trees between local and remote endpoints. |
| `forward_ports` | Open, list, or close TCP/UDP forwards between two sides. |

Default-hidden file tools are available only when explicitly enabled under
`[tools.file]` in broker config:

- `read`
- `write`
- `edit`

## Components

| Component | Role |
| --- | --- |
| `remote-exec-broker` | Public MCP server over stdio or streamable HTTP. It validates targets, routes calls, owns public `session_id` and `forward_id` namespaces, and can use the broker host as `local`. |
| `remote-exec` | CLI client built from the broker crate. It can run against a config in-process or a streamable-HTTP broker. |
| `remote-exec-daemon` | Rust per-machine daemon. It supports mutual TLS by default, optional plain HTTP, exec, patch, image, transfer, sandbox checks, and v4 port-forward tunnels. |
| `remote-exec-host` | Shared Rust host runtime reused by the Rust daemon and broker-host `local` behavior. |
| `remote-exec-daemon-cpp` | Standalone C++11 daemon with optional OpenSSL mTLS, explicit plain HTTP, and POSIX and legacy Windows build paths. |
| `remote-exec-proto` | Public MCP schemas, broker-daemon RPC schemas, path and sandbox helpers, and port-forward protocol types. |
| `remote-exec-admin` | Certificate/bootstrap CLI. |
| `remote-exec-pki` | Reusable certificate generation, manifest, and private-key write helpers. |

## Runtime Model

Agents talk only to the broker. Each configured remote target points at one
daemon. The broker performs target validation, identity checks, routing, result
formatting, and runtime ID mapping.

Important invariants:

- Target selection is part of the security boundary.
- Configured remote targets may not be named `local`.
- A session, file operation, or forward opened for one target is not valid for
  another target.
- Public `session_id` and `forward_id` values are broker-owned opaque tokens.
  They are not daemon process IDs, daemon-local session IDs, tunnel IDs, or
  stream IDs.
- `list_targets` reports configured targets and cached daemon metadata. It may
  perform a short bounded status refresh for targets without currently
  available metadata, but it does not expose stale daemon metadata while a
  target is unavailable.
- Public `supports_port_forward` means the target reported forwarding support
  and the broker verified a supported tunnel protocol version.
- Temporarily unreachable targets can remain configured. Broker startup may
  succeed and verify a target before the first forwarded call.
- Broker-daemon RPC uses HTTP/1.1 JSON. Port forwarding uses daemon-private
  HTTP/1.1 Upgrade tunnels.
- Direct targets and daemon-initiated reverse targets preserve the same public
  MCP behavior and broker-owned ID namespaces.
- `forward_ports` v4 uses `X-Remote-Exec-Port-Tunnel-Version: 4`. Frame numbers
  20 and 21 are reserved as `ForwardRecovering` and `ForwardRecovered`; public
  recovery state is reported through broker-owned `forward_ports list` fields.

Live exec sessions and live port forwards are in-memory runtime state. Broker
restart drops public `session_id` and `forward_id` mappings. Daemon restart
drops daemon-local command sessions and forwarded sockets.

## Configuration

Use the example configs as the canonical shape:

| File | Covers |
| --- | --- |
| `configs/broker.example.toml` | MCP transport, targets, TLS client credentials, reverse listener, broker-host `local`, host sandbox, hidden file tools, transfer limits, and forwarding limits. |
| `configs/daemon.example.toml` | Rust daemon target name, listen/reverse mode, default workdir, TLS or HTTP transport, bearer auth, shell/PTY policy, sandbox, transfer, yield-time, and forwarding limits. |
| `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini` | C++ daemon listen address, direct/reverse TLS or HTTP transport, default workdir, bearer auth, request/session/forwarding limits, yield-time limits, and sandbox. |

Common rules:

- `default_workdir` must already exist when a broker `[local]` target or daemon
  starts.
- Rust broker and Rust daemon targets use TLS by default.
- Plain HTTP targets must be configured with `allow_insecure_http = true` in the
  broker.
- The C++ daemon defaults to `TLS=auto`. POSIX builds enable TLS when OpenSSL
  1.0.2 or newer is available; Windows XP and newer targets enable it
  automatically, while older Windows targets default to TLS off.
- Plain C++ daemon targets require `allow_insecure_http = true` and should
  usually use bearer auth when they are not isolated by another trusted
  transport.
- Reverse mode keeps a bounded adaptive lane pool because transfers and
  port-forward upgrades can occupy HTTP/1.1 connections for long periods.
- Both daemons support reverse TLS and explicit insecure HTTP. The C++ daemon
  requires an OpenSSL-enabled build for TLS.

## TLS And Bootstrap

Rust broker and Rust daemon targets use mutual TLS by default:

- broker feature `broker-tls` is enabled by default
- daemon feature `tls` is enabled by default
- the daemon presents a server certificate signed by the configured CA
- the broker presents a client certificate signed by the configured CA
- both sides trust the configured CA

If the broker is built without `broker-tls`, it rejects `https://` daemon
targets and `https://` broker URLs. If the Rust daemon is built without `tls`,
it only supports `transport = "http"`.

Generated broker and daemon leaf certificates are usable for both direct and
reverse TLS. The broker leaf has client-auth usage for direct daemon requests
and server-auth usage for the reverse listener, with a DNS SAN matching the
broker common name. Each daemon leaf has server-auth usage for direct mode and
client-auth usage for reverse lane registration, with its common name matching
the configured target. Plain reverse mode requires the configured bearer token
for lane registration and normal RPC authentication.

Development bootstrap:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs \
  --target builder-a \
  --target builder-b
```

Reuse an existing CA:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs-next \
  --target builder-c \
  --reuse-ca-from-dir ./remote-exec-certs
```

Add daemon SANs when the broker connects by DNS name or non-localhost IP:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs \
  --target builder-a \
  --san builder-a=dns:builder-a.example.com \
  --san builder-a=ip:10.0.0.12
```

`dev-init` writes `ca.pem`, `ca.key`, `broker.pem`, `broker.key`,
`daemons/<target>.pem`, `daemons/<target>.key`, reverse-mode certificates under
`reverse/`, and `certs-manifest.json`.

Certificate notes:

- If no SAN is provided, generated daemon certs default to `DNS:localhost` and
  `IP:127.0.0.1`.
- Generated private keys are written with restricted permissions: Unix `0600`;
  Windows DACL for current user, local Administrators, and LocalSystem.
- `expected_daemon_name` should match the daemon's configured `target`.
- `skip_server_name_verification = true` still validates CA, key usage, and
  expiry, but skips URL host to certificate SAN matching.
- `pinned_server_cert_pem` and `tls.pinned_client_cert_pem` add exact leaf
  certificate pins on top of normal CA validation.
- Bearer auth authenticates requests but does not add confidentiality or
  integrity on plain HTTP.

## Running

Start a Rust daemon:

```bash
cargo run -p remote-exec-daemon -- configs/daemon.example.toml
```

Start the broker over stdio:

```bash
cargo run -p remote-exec-broker -- configs/broker.example.toml
```

Expose the broker over streamable HTTP:

```toml
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "/mcp"
```

Run the C++ daemon:

```bash
make -C crates/remote-exec-daemon-cpp
crates/remote-exec-daemon-cpp/build/remote-exec-daemon-cpp \
  crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini
```

## CLI Client

The `remote-exec` CLI calls the same public broker tools.

Use a broker config in-process:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  list-targets
```

Use a running streamable-HTTP broker:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-url http://127.0.0.1:8787/mcp \
  list-targets
```

Common examples:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  exec --target builder-a --workdir /srv/project 'cargo test'

cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  transfer-files \
  --source local:/tmp/source.txt \
  --destination builder-a:/tmp/dest.txt \
  --overwrite replace \
  --create-parent

cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-url http://127.0.0.1:8787/mcp \
  forward-ports open \
  --listen-side local \
  --connect-side builder-a \
  --forward tcp:127.0.0.1:15432=127.0.0.1:5432
```

Use `--json` for normalized JSON output. Use `apply-patch --input-file -` and
`write-stdin --chars-file -` to read payloads from stdin.

CLI exit codes are:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `2` | Usage or input error. |
| `3` | Broker config load/build error. |
| `4` | Streamable-HTTP connection or transport error. |
| `5` | MCP tool error returned by the broker. |

`--broker-config` mode builds broker state for one CLI invocation. Persistent
port forwards require a long-running broker, so prefer `--broker-url` for
`forward-ports open/list/close` workflows.

## Tool Behavior

`exec_command`:

- runs one command on one target
- returns `session_id` when still running
- merges stdout/stderr order into one public `output` field for non-TTY exec
- applies daemon or broker-local `yield_time_ms` policy
- truncates output by approximate token budget, where one token is about four
  UTF-8 bytes

`write_stdin`:

- writes to or polls a broker-owned live session
- can route by `session_id` alone
- rejects mismatched `target` if supplied
- accepts `pty_size` for live TTY resize before writing or polling
- normalizes lost daemon sessions into the usual unknown-process error

`apply_patch`:

- applies Codex-style patches on one target
- preserves existing `LF` versus `CRLF` style for updated files
- supports the documented `*** End of File` marker
- preflights deterministic failures such as sandbox denial, missing files,
  non-file targets, decode failures, and unmatched hunks before writing
- remains non-transactional for runtime races and write/remove failures after
  preflight
- can use experimental target encoding autodetection when enabled in config

Default-hidden file tools:

- `read` reads one text file and returns text output only
- `read.file_path`, `write.file_path`, and `edit.file_path` resolve relative to
  the target default workdir unless they are target-native absolute paths
- `read.offset` is one-based; omitted or `0` means line `1`
- `read.limit` defaults to `tools.file.default_read_limit_lines`
- `write` overwrites an existing file or creates a new file
- `edit` replaces `old_string` with `new_string`; if `old_string` matches more
  than once, the call fails unless `replace_all = true`
- these tools are not listed or callable unless enabled under `[tools.file]`

`view_image`:

- reads an image from one target
- accepts `detail` for compatibility but does not use it
- preserves PNG, JPEG, and WebP bytes without resizing
- Rust daemon transcodes BMP, GIF, ICO, PNM (including PPM), and TGA to PNG;
  the C++ daemon supports passthrough PNG, JPEG, and WebP only

`transfer_files`:

- supports `local -> remote`, `remote -> local`, `remote -> remote`, and
  `local -> local`
- accepts either one `source` or a `sources` array
- requires endpoint-native absolute paths
- defaults to `overwrite = "merge"`, `destination_mode = "auto"`, and
  `symlink_mode = "preserve"`
- supports `overwrite = "fail"`, `"merge"`, and `"replace"`
- supports `destination_mode = "exact"` and `"into_directory"`
- supports `symlink_mode = "preserve"`, `"follow"`, and `"skip"`
- supports `exclude` glob patterns relative to each source root
- skips unsupported special files inside directory trees with warnings
- does not expose a public compression option; compression is broker-internal
- is not transactional; failed transfers can leave partial destination changes

`forward_ports`:

- supports `action = "open" | "list" | "close"`
- supports `tcp` and `udp`
- opens listeners on `listen_side` and outbound connections/datagrams on
  `connect_side`
- allows either side to be a configured target or `"local"`
- treats bare endpoint `"8080"` as `"127.0.0.1:8080"`
- allows non-loopback listen binds such as `"0.0.0.0:8080"`
- allows `listen_endpoint` port `0`; read the result for the actual bound port
- requires nonzero `connect_endpoint` ports
- reports `phase`, side health, generation, reconnect counters, drop counters,
  and effective limits
- treats `phase = "ready"` as readiness; legacy `status = "open"` can coexist
  with `phase = "reconnecting"`
- can recover future listen-side traffic after broker-daemon transport loss
  when the daemon stays alive, but active TCP streams and UDP per-peer connector
  state are lost

## Local Semantics

The name `local` means the broker host.

- `[local]` enables `target: "local"` for `exec_command`, `write_stdin`,
  `apply_patch`, `view_image`, and enabled default-hidden file tools.
- `transfer_files` can use `target: "local"` for broker-host filesystem access
  even when `[local]` is omitted.
- `forward_ports` can use side `"local"` for broker-host network access even
  when `[local]` is omitted.
- Broker `host_sandbox` governs broker-host filesystem access. It does not
  restrict `forward_ports` network access.

## Trust Model

Selecting a target grants broad access on that machine unless static sandbox
config restricts the relevant path-based operation. There is no per-call
approval flow and no sandbox selection flow.

Sandbox rules are static allow/deny lists:

- missing `allow` or `allow = []` means allow all
- `deny` entries refine the allowed set
- `exec_command` checks only the resolved starting `cwd`
- command text is not inspected for arbitrary path references
- `view_image` and `read` check read paths
- `write`, `edit`, and `apply_patch` check write paths
- `transfer_files` checks source read access and destination write access on
  their respective endpoints
- `forward_ports` can bind non-loopback addresses and connect to arbitrary
  endpoints reachable from each side, subject to configured forwarding limits

Security is based on explicit target selection plus broker-to-daemon mutual TLS
for normal Rust targets. Plain HTTP requires explicit opt-in.

## Reliability Notes

- Broker startup probes run concurrently and are bounded by
  `timeouts.startup_probe_ms`.
- Broker target health refresh runs periodically in the background with
  separate healthy and unhealthy intervals.
- Timed-out broker-daemon requests are never replayed.
- After repeated direct-target timeouts without a successful HTTP response, the
  broker replaces the target's HTTP client pool. Reverse targets additionally
  discard queued idle lanes so the daemon replenishes them.
- `exec_command` and `write_stdin` use at least `yield_time_ms` plus a small
  slow-host margin for their daemon RPC timeout.
- Rust daemon and broker-host live exec sessions are capped by
  `max_open_sessions` and prune older sessions under pressure, preferring
  completed sessions.
- `forward_ports open` is all-or-nothing for a single tool call.
- Explicit `forward_ports close` reports an error if daemon-side cleanup cannot
  be confirmed, leaving listed state available for retry or inspection.
- If the broker disappears without closing a forward, daemon-side detached
  listeners and UDP sockets are reclaimed after the reconnect grace window.
- Rust daemon shutdown cancels pending tunnel work and closes live forwarded
  sockets before exit.
- C++ daemon forwarding bounds worker count, tunnel I/O, queued bytes, UDP
  binds, active TCP streams, retained sessions/listeners, and TCP connect time.

## C++ Daemon

The C++ daemon intentionally supports a smaller surface than the Rust daemon:

| Area | C++ daemon behavior |
| --- | --- |
| Transport | Optional OpenSSL direct/reverse mTLS, or explicit plain HTTP. TLS builds use `https://...`; plain targets use `http://...` and `allow_insecure_http = true`. |
| Security | Mutual TLS with OpenSSL 1.0.2 or newer, optional bearer auth, certificate pinning, and static path sandboxing. |
| Exec | C++11 implementation for POSIX and legacy Windows hosts. POSIX PTY is supported when the host can allocate one. GNU/MSVC Windows PTY support depends on vendored `winpty`. |
| Files | `apply_patch`, `view_image` passthrough for PNG/JPEG/WebP, and transfer import/export. Default-hidden `read` / `write` / `edit` are not implemented yet. |
| Transfers | File, directory, and broker-built multi-source transfers. No transfer compression. |
| Forwarding | v4 TCP/UDP port-forward tunnel support with daemon-local worker and queue limits. |
| Legacy Windows | GNU paths cover NT 3.x Winsock 1.1, NT 4.0 Winsock 1.1/2, Windows 2000, XP, x64 NT-family, and ANSI Windows 9x/Me variants. MSVC covers native and `v141_xp` XP-compatible paths. |

The daemon builds as C++11 on every supported build path. In this repository,
"Windows XP-compatible" means using a toolchain that can target XP while still
compiling the daemon as C++11.

Common C++ checks:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-2000
make -C crates/remote-exec-daemon-cpp prepare-openssl-xp OPENSSL_DEPS_DIR=/path/to/deps
make -C crates/remote-exec-daemon-cpp check-windows-xp OPENSSL_ROOT=/path/to/deps/openssl-3.5.7
make -C crates/remote-exec-daemon-cpp check-windows-x64 OPENSSL_ROOT=/path/to/openssl-mingw64
make -C crates/remote-exec-daemon-cpp check-windows-nt3x-ws1
make -C crates/remote-exec-daemon-cpp check-windows-nt4-ws1
make -C crates/remote-exec-daemon-cpp check-windows-nt4-ws2
bmake -C crates/remote-exec-daemon-cpp check-posix
```

From an x86 Visual Studio developer prompt:

```bat
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-native OPENSSL_ROOT=C:\path\to\openssl
```

From an x86 Visual Studio developer prompt with a `v141_xp`-capable C++11
toolset:

```bat
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-xp OPENSSL_ROOT=C:\path\to\openssl-xp
```

More C++ daemon details live in
`crates/remote-exec-daemon-cpp/README.md`.

## Observability

Runtime components log to `stderr`.

- The broker keeps `stdout` reserved for MCP stdio.
- Broker tool errors include `request_id`, `tool`, and `target` when known.
- Rust components read `REMOTE_EXEC_LOG` first, then `RUST_LOG`.
- C++ daemon reads `REMOTE_EXEC_LOG` first, then `RUST_LOG`.
- C++ daemon accepts a bare level such as `debug`, shared filters such as
  `remote_exec_daemon_cpp=debug`, and the old
  `remote_exec_daemon_xp=<level>` alias.

Examples:

```bash
REMOTE_EXEC_LOG=debug cargo run -p remote-exec-daemon -- configs/daemon.example.toml
REMOTE_EXEC_LOG=debug cargo run -p remote-exec-broker -- configs/broker.example.toml
REMOTE_EXEC_LOG='warn,remote_exec_broker=debug,remote_exec_daemon=debug,remote_exec_daemon_cpp=debug'
```

## Development

Rust MSRV is `1.85.0`, the first stable release with Rust 2024 edition support.

Full quality gate:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
crates/remote-exec-daemon-cpp/scripts/clang_format.sh check
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-2000
make -C crates/remote-exec-daemon-cpp check-windows-xp
make -C crates/remote-exec-daemon-cpp check-windows-x64
make -C crates/remote-exec-daemon-cpp check-windows-nt3x-ws1
make -C crates/remote-exec-daemon-cpp check-windows-nt4-ws1
make -C crates/remote-exec-daemon-cpp check-windows-nt4-ws2
```

Additional platform checks:

```bat
make -C crates/remote-exec-daemon-cpp check-windows WINDOWS_TOOLCHAIN=native WINDOWS_WINVER=0x0501 WINDOWS_WINSOCK_VERSION=2
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-native
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-xp
```

Focused commands:

```bash
cargo test -p remote-exec-broker --test multi_target -- --nocapture
cargo test -p remote-exec-broker --test mcp_cli
cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture
cargo test -p remote-exec-broker --test mcp_forward_ports -- --nocapture
cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture
cargo test -p remote-exec-daemon --test port_forward_rpc -- --nocapture
make -C crates/remote-exec-daemon-cpp test-host-transfer
make -C crates/remote-exec-daemon-cpp test-host-server-runtime
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp test-windows-xp-server-runtime
make -C crates/remote-exec-daemon-cpp test-windows-xp-server-routes-common
```

C++ daemon sources are formatted with the root `.clang-format`:

```bash
crates/remote-exec-daemon-cpp/scripts/clang_format.sh format
crates/remote-exec-daemon-cpp/scripts/clang_format.sh check
```

No-default-features checks:

```bash
cargo test -p remote-exec-broker --no-default-features --tests
cargo test -p remote-exec-daemon --no-default-features --tests
cargo test -p remote-exec-host --no-default-features --tests
cargo clippy -p remote-exec-broker --no-default-features --all-targets -- -D warnings
cargo clippy -p remote-exec-daemon --no-default-features --all-targets -- -D warnings
cargo clippy -p remote-exec-host --no-default-features --all-targets -- -D warnings
```

Windows GNU compile-only checks from Linux:

```bash
cargo check --workspace --all-targets --all-features --target x86_64-pc-windows-gnu
cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings
cargo build --workspace --all-targets --all-features --target x86_64-pc-windows-gnu
```

CI covers Rust broker/daemon paths on Linux and Windows, standalone C++ daemon
jobs on Linux and Windows, C++ formatting, selected Wine runs for legacy GNU
Windows binaries when available, and periodic/manual BSD coverage using BSD
make. Rust broker integration tests consume a prebuilt C++ daemon binary when
one is present and skip C++ daemon scenarios when it is absent.

## References

- `AGENTS.md`: implementation guidance for coding agents
- `skills/using-remote-exec-mcp/SKILL.md`: tool and CLI usage guide for agents
- `configs/broker.example.toml`: broker config shape
- `configs/daemon.example.toml`: Rust daemon config shape
- `crates/remote-exec-daemon-cpp/README.md`: C++ daemon build/runtime guide
- `crates/remote-exec-proto/src/public.rs`: public MCP tool schema
