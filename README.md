# remote-exec-mcp

`remote-exec-mcp` is a remote-first MCP server for running Codex-style local
system tools on multiple Linux and Windows machines. Agents connect to one
broker, choose an explicit target, and use familiar tools for command
execution, stdin, patching, image reads, file transfer, and TCP/UDP forwarding.

The tool interface and behavior are influenced by
[Codex](https://github.com/openai/codex), but this repository is a separate
broker plus per-machine daemon implementation.

Everything under `docs/` is historical implementation detail and planning
context, not the live behavior contract. Current source of truth:

- this `README.md`
- `AGENTS.md`
- `configs/*.example.toml`
- `skills/using-remote-exec-mcp/SKILL.md`
- public schemas in `crates/remote-exec-proto/src/`

## Status

Implemented public MCP tools:

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`
- `forward_ports`

Implemented transports and runtimes:

- broker MCP over stdio or streamable HTTP
- Rust daemon over mutual TLS by default, or explicit plain HTTP
- broker-host `local` target for embedded local exec/patch/image workflows
- broker-host `local` filesystem endpoint for transfers
- broker-host `local` network side for port forwards
- `remote-exec` CLI client for broker config or streamable HTTP use
- standalone C++ daemon for POSIX and Windows XP-compatible hosts

Live exec sessions and live port forwards are in-memory runtime state. Broker
restart drops public `session_id` and `forward_id` mappings. Daemon restart
drops daemon-local command sessions and forwarded sockets.

## Components

- `remote-exec-broker`: public MCP server. It validates target names, routes
  calls to daemons or broker-host local runtime, owns public `session_id` and
  `forward_id` namespaces, and can serve MCP over stdio or streamable HTTP.
- `remote-exec`: CLI client built from the broker crate. It can load a broker
  config and call handlers in-process, or connect to a running streamable-HTTP
  broker.
- `remote-exec-daemon`: Rust per-machine daemon. It executes commands, manages
  sessions, applies patches, reads images, imports/exports transfer archives,
  checks static path sandbox rules, and serves v4 port-forward upgrade tunnels.
- `remote-exec-host`: shared Rust host runtime reused by the Rust daemon and the
  broker-host `local` target.
- `remote-exec-daemon-cpp`: standalone plain-HTTP C++ daemon with native POSIX,
  MinGW Windows XP-compatible, host-native MSVC, and MSVC v141_xp-compatible
  build paths.
- `remote-exec-proto`: public MCP schemas, broker-daemon RPC schemas, path and
  sandbox helpers, and port-forward protocol types.
- `remote-exec-admin`: administrative CLI for certificate/bootstrap workflows.
- `remote-exec-pki`: reusable PKI generation and manifest helpers.

## Architecture

Agents talk only to the broker. Each configured target points at one daemon, and
the broker performs target validation, identity checks, routing, result
formatting, and runtime ID mapping.

Important invariants:

- `list_targets` is broker-local cached inventory. It does not probe daemons at
  read time.
- A temporarily unreachable target can still be configured. The broker may start
  successfully and verify that target before the first forwarded call.
- Public `session_id` values are broker-owned opaque tokens, not daemon process
  IDs or daemon-local session IDs.
- Public `forward_id` values are broker-owned opaque tokens, not daemon-local
  tunnel IDs.
- Target selection is part of the security boundary. A session or forward opened
  for one target is not valid for another target.
- Broker-daemon RPC uses HTTP/1.1 JSON. Port forwarding uses daemon-private
  HTTP/1.1 Upgrade tunnels.
- `forward_ports` v4 uses `X-Remote-Exec-Port-Tunnel-Version: 4`. Header
  matching is case-insensitive; the protocol version is `4`.
- v4 frame numbers 20 and 21 are reserved as `ForwardRecovering` and
  `ForwardRecovered`. Public recovery state is currently reported through
  broker-owned `forward_ports list` fields.

## Configuration

Start with:

- `configs/broker.example.toml`
- `configs/daemon.example.toml`
- `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`

Broker config covers:

- MCP transport: stdio by default, or streamable HTTP with `listen` and `path`
- one `[targets.<name>]` entry per daemon target
- daemon base URL and expected daemon target name
- mutual TLS client cert/key and CA paths for `https://` targets
- explicit `allow_insecure_http = true` for plain-HTTP targets
- optional bearer auth for broker-to-daemon requests
- optional certificate pinning and hostname-verification override
- per-target connect/read/request/startup probe timeouts
- optional broker-host `[local]` target
- optional broker-host filesystem sandbox
- optional structured-content toggle
- optional transfer and port-forward limits

Daemon config covers:

- daemon `target` name and `listen` address
- default working directory
- TLS transport by default, or explicit `transport = "http"`
- TLS server cert/key and CA paths
- optional broker client certificate pin
- optional bearer auth
- login-shell, PTY, default-shell, and Windows POSIX-root policy
- optional static path sandbox
- optional transfer, yield-time, and port-forward limits

`default_workdir` must already exist when a broker `[local]` target or daemon
starts.

## TLS And Bootstrap

Rust broker and Rust daemon targets use mutual TLS by default:

- broker feature `broker-tls` is enabled by default
- daemon feature `tls` is enabled by default
- the daemon presents a server certificate signed by the configured CA
- the broker presents a client certificate signed by the configured CA
- both sides trust the CA configured in their config

If the broker is built without `broker-tls`, it rejects `https://` daemon
targets and `https://` broker URLs. If the Rust daemon is built without `tls`,
it only supports `transport = "http"`. C++ daemon targets are plain HTTP and
must be configured in the broker with `allow_insecure_http = true`.

Preferred development bootstrap:

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

The command writes:

- `ca.pem` and `ca.key`
- `broker.pem` and `broker.key`
- `daemons/<target>.pem` and `daemons/<target>.key`
- `certs-manifest.json`

Lower-level commands are also available:

```bash
cargo run -p remote-exec-admin -- certs init-ca --out-dir ./remote-exec-ca

cargo run -p remote-exec-admin -- certs issue-broker \
  --ca-cert-pem ./remote-exec-ca/ca.pem \
  --ca-key-pem ./remote-exec-ca/ca.key \
  --out-dir ./remote-exec-broker-cert

cargo run -p remote-exec-admin -- certs issue-daemon \
  --ca-cert-pem ./remote-exec-ca/ca.pem \
  --ca-key-pem ./remote-exec-ca/ca.key \
  --out-dir ./remote-exec-daemon-cert \
  --target builder-a \
  --san dns:builder-a.example.com \
  --san ip:10.0.0.12
```

Notes:

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

Start the broker:

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

CLI exit codes are `0` for success, `2` for usage/input errors, `3` for broker
config load/build errors, `4` for streamable-HTTP connection or transport
errors, and `5` for MCP tool errors returned by the broker.

`--broker-config` mode builds broker state for one CLI invocation. Persistent
port forwards require a long-running broker, so prefer `--broker-url` for
`forward-ports open/list/close` workflows.

## Tool Notes

`exec_command`:

- runs one command on one target
- returns `session_id` when still running
- merges stdout/stderr order into one public `output` field for non-TTY exec
- applies daemon or broker-local `yield_time_ms` policy
- truncates output by approximate token budget, where one token is about four
  UTF-8 bytes
- reports warnings in text and, when enabled, structured content

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
- is intentionally non-transactional across multiple file actions
- returns text output only
- can use experimental target encoding autodetection when enabled in config

`view_image`:

- reads an image from one target
- supports `detail = "original"` for full-fidelity reads
- Rust daemon can resize/default according to normal image handling
- C++ daemon supports passthrough PNG, JPEG, and WebP only

`transfer_files`:

- supports `local -> remote`, `remote -> local`, `remote -> remote`, and
  `local -> local`
- accepts either one `source` or a `sources` array
- requires endpoint-native absolute paths
- defaults to `overwrite = "merge"`, `destination_mode = "auto"`, and
  `symlink_mode = "preserve"`
- supports `destination_mode = "exact"` and `"into_directory"`
- supports `symlink_mode = "preserve"`, `"follow"`, and `"skip"`
- supports `exclude` glob patterns relative to each source root
- skips unsupported special files inside directory trees with warnings
- does not expose a public compression option; compression is broker-internal

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
- can recover future listen-side traffic after broker-daemon transport loss when
  the daemon stays alive, but active TCP streams and UDP per-peer connector state
  are lost

## Local Semantics

The name `local` means the broker host.

- `[local]` in broker config enables `target: "local"` for `exec_command`,
  `write_stdin`, `apply_patch`, and `view_image`.
- `transfer_files` can use `target: "local"` for broker-host filesystem access
  even when `[local]` is omitted.
- `forward_ports` can use side `"local"` for broker-host network access even
  when `[local]` is omitted.
- Broker `host_sandbox` governs broker-host filesystem access. It does not
  restrict `forward_ports` network access.

Configured remote targets may not be named `local`.

## Trust Model

Selecting a target is equivalent to broad access on that machine unless static
sandbox config restricts the relevant path-based operation.

There is no per-call approval flow and no sandbox selection flow. Sandbox rules
are static allow/deny lists:

- missing `allow` or `allow = []` means allow all
- `deny` entries refine the allowed set
- `exec_command` checks only the resolved starting `cwd`
- command text is not inspected for arbitrary path references
- `view_image` checks the resolved image path for read access
- `apply_patch` checks resolved write targets
- `transfer_files` checks source read access and destination write access on
  their respective endpoints
- `forward_ports` can bind non-loopback addresses and connect to arbitrary
  endpoints reachable from each side, subject to configured forwarding limits

Security is based on explicit target selection plus broker-to-daemon mutual TLS
for normal Rust targets. Plain HTTP requires explicit opt-in.

## Reliability Notes

- Broker startup probes run concurrently and are bounded by
  `timeouts.startup_probe_ms`.
- Broker-daemon calls are bounded by per-target connect/read/request timeouts.
- Rust daemon live exec sessions are capped by `max_open_sessions` and prune
  older sessions under pressure, preferring completed sessions. Broker-host
  local runtime uses the same default cap.
- `forward_ports` open is all-or-nothing for a single tool call: failed
  initialization closes listeners created during that call.
- Explicit `forward_ports` close reports an error if daemon-side cleanup cannot
  be confirmed, leaving listed state available for retry or inspection.
- If the broker disappears without closing a forward, daemon-side detached
  listeners and UDP sockets are reclaimed after the reconnect grace window.
- Rust daemon shutdown cancels pending tunnel work and closes live forwarded
  sockets before exit.
- C++ daemon forwarding bounds worker count, tunnel I/O, queued bytes, UDP
  binds, active TCP streams, retained sessions/listeners, and TCP connect time.

## C++ Daemon

The C++ daemon intentionally supports a smaller surface than the Rust daemon:

- plain HTTP only
- no transfer compression
- POSIX PTY support when the host can allocate a PTY
- Windows XP-compatible builds reject `tty = true`
- PNG, JPEG, and WebP passthrough `view_image`
- file, directory, and broker-built multi-source transfers
- POSIX symlink preserve/follow/skip modes
- Windows XP-compatible symlink preservation skipped when unavailable
- v4 `forward_ports` tunnel support
- static path sandboxing for exec cwd, transfer read/write paths, patch write
  targets, and image reads

The Rust and C++ daemons share the `max_open_sessions` default of 64. C++ also
has daemon-local safety knobs for its handwritten HTTP parser and blocking
forwarding worker model: `max_request_header_bytes`, `max_request_body_bytes`,
`port_forward_max_worker_threads`, and `port_forward_tunnel_io_timeout_ms`.
Those are deliberately C++-specific rather than hidden Rust equivalents.

Build paths:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
bmake -C crates/remote-exec-daemon-cpp check-posix
```

From an x86 Visual Studio developer prompt:

```bat
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-native
```

From an x86 Visual Studio developer prompt with the v141_xp-capable toolset:

```bat
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-xp
```

More C++ daemon details live in `crates/remote-exec-daemon-cpp/README.md`.

## Observability

Runtime components log to `stderr`.

- The broker keeps `stdout` reserved for MCP stdio.
- Broker tool errors include `request_id`, `tool`, and `target` when known;
  use that request ID to correlate broker logs with daemon `x-request-id` logs.
- Rust components read `REMOTE_EXEC_LOG` first, then `RUST_LOG`.
- C++ daemon reads `REMOTE_EXEC_LOG` first, then `RUST_LOG`.
- C++ daemon accepts a bare level such as `debug`, and shared filters such as
  `remote_exec_daemon_cpp=debug`.
- The old `remote_exec_daemon_xp=<level>` filter remains accepted as an alias.

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
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
# From an x86 Visual Studio developer prompt:
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-native
# From an x86 Visual Studio developer prompt with the v141_xp-capable toolset:
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
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
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

CI also exercises broker, daemon, and host `--no-default-features` test and
clippy jobs on Ubuntu so the `tls-disabled` and host feature-gated code paths
stay intentionally covered.

CI exercises the Rust broker and Rust daemon on Linux and Windows. The Rust
broker integration tests consume a prebuilt C++ daemon binary when one is
present, and skip the C++ daemon scenarios when it is absent; they do not build
the C++ daemon themselves. CI builds that C++ daemon binary in an explicit step
before the Rust test job. The standalone C++ daemon also has its own Linux and
Windows CI job: POSIX runtime tests run on Linux, Windows XP-compatible test
binaries run under Wine on Linux when available, and the 32-bit host-native MSVC
NMAKE path runs on `windows-latest`.

Windows GNU compile-only checks from Linux:

```bash
cargo check --workspace --all-targets --all-features --target x86_64-pc-windows-gnu
cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings
cargo build --workspace --all-targets --all-features --target x86_64-pc-windows-gnu
```

## References

- `AGENTS.md`: implementation guidance for coding agents
- `skills/using-remote-exec-mcp/SKILL.md`: tool and CLI usage guide for agents
- `configs/broker.example.toml`: broker config shape
- `configs/daemon.example.toml`: Rust daemon config shape
- `crates/remote-exec-daemon-cpp/README.md`: C++ daemon build/runtime guide
- `crates/remote-exec-proto/src/public.rs`: public MCP tool schema
