# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux and Windows machines.

The tool interfaces and behavior in this project are heavily influenced by [Codex](https://github.com/openai/codex), while the implementation here is a separate remote-first broker and per-machine daemon design.

Everything under `docs/` is historical implementation detail and planning context, not the live behavior contract. Treat this `README.md`, `AGENTS.md`, the config examples, and `skills/using-remote-exec-mcp/SKILL.md` as the current source of truth.

## Components

- `remote-exec-admin`
  - Administrative CLI for TLS bootstrap and future operator workflows.
- `remote-exec-broker`
  - Public MCP server over stdio by default, or over streamable HTTP when configured.
  - The `broker-tls` Cargo feature gates broker-side `https://` client support for daemon targets and `https://` broker URLs, and is enabled by default.
  - Accepts tool calls with a required `target` for machine-local operations.
  - Owns opaque public `session_id` values for live command sessions.
  - Owns opaque public `forward_id` values for live TCP/UDP port forwards.
  - Can optionally expose the broker host itself as `target: "local"` for daemon-backed `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
  - Always provides broker-host filesystem access for `transfer_files` endpoints that use `target: "local"`, even when the broker `[local]` target is disabled.
  - Always provides broker-host network access for `forward_ports` sides that use `"local"`, even when the broker `[local]` target is disabled.
- `remote-exec`
  - CLI client for the broker's public MCP tool surface.
  - Can load a broker config and invoke the broker tool handlers directly, or connect to a broker streamable HTTP endpoint.
- `remote-exec-daemon`
  - Per-machine daemon over mTLS JSON/HTTP by default, or plain HTTP when configured.
  - The `tls` Cargo feature gates the HTTPS/mTLS transport and is enabled by default.
  - Executes commands, manages local sessions, applies patches, reads images, serves transfer archives, and provides broker-controlled port-forward socket primitives.
- `remote-exec-host`
  - Shared Rust host runtime used by broker-host `local` behavior and the Rust daemon.
  - Owns transport-neutral host config, path handling, and the extracted host-local capability runtime as that boundary is cleaned up.
- `remote-exec-daemon-cpp`
  - Standalone C++ daemon over plain HTTP, with native POSIX and Windows XP-compatible MinGW/MSVC build paths.
  - Supports `exec_command`, `write_stdin`, `apply_patch`, `transfer_files` for files/directories/broker-built multi-source bundles, and `forward_ports`.
  - Supports POSIX PTY sessions when the host can allocate a PTY; Windows XP-compatible PTY sessions and image reads remain unsupported.
  - Intentionally closes stdin for POSIX non-PTY exec, while keeping Windows XP-compatible non-PTY stdin open for original-daemon compatibility.
- `remote-exec-proto`
  - Shared public tool schemas and broker-daemon RPC types.

## Supported tools

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`
- `forward_ports`

## Architecture

- Agents talk only to the broker.
- The broker can expose MCP over stdio or streamable HTTP.
- Agents can call `list_targets` to discover configured logical target names and cached daemon metadata when available.
- When broker `[local]` config is enabled, `list_targets` also includes `local` for the broker host.
- `list_targets` is broker-local and does not probe daemons at read time.
- `list_targets` reports both `forward_ports=yes|no` and, when forwarding is supported, `forward_protocol=vN` for the cached broker-daemon port-forward protocol version on that target.
- The broker validates `target`, forwards the request to the selected daemon, and returns MCP-compatible content plus structured JSON for tools that expose it unless `disable_structured_content = true` is configured.
- Broker-daemon RPC uses HTTP/1.1 only. The broker keeps daemon connections eligible for client-side pooling instead of forcing every request to close the TCP connection.
- For the optional `local` target, the broker reuses the shared Rust host runtime in-process instead of asking operators to run a second same-host daemon manually.
- Each daemon serves exactly one configured target machine.
- Live exec sessions are broker-routed by opaque public `session_id`, not by daemon-local process identifiers.

## Configuration

Example configs live in:

- `configs/broker.example.toml`
- `configs/daemon.example.toml`

Daemon config covers:

- target name
- listen address
- daemon transport: mutual TLS by default when built with the default `tls` Cargo feature, or explicit plain HTTP
- optional HTTP bearer auth shared secret via `Authorization: Bearer ...`
- optional exact broker leaf certificate pin for TLS mode
- default working directory
- optional static sandbox allow/deny rules for exec `cwd`, reads, and writes
- optional transfer compression support toggle
- optional default shell override
- optional PTY mode selection
- optional per-operation yield-time policy overrides for `exec_command`, empty `write_stdin` polls, and non-empty `write_stdin` writes
- TLS certificate, key, and CA paths

Broker config covers one entry per target:

- optional MCP transport selection
- optional broker-host sandbox allow/deny rules for exec `cwd`, reads, and writes
- optional broker-side transfer compression support toggle
- optional broker-side MCP structured-content toggle
- daemon base URL
- `https://` daemon targets when the broker is built with the default `broker-tls` Cargo feature, or explicit plain `http://` targets with `allow_insecure_http = true`
- CA path for `https://` targets
- client certificate path for `https://` targets
- client key path for `https://` targets
- optional `skip_server_name_verification = true` for `https://` targets that should validate chain and expiry but ignore SAN/hostname matching
- optional exact leaf certificate pin via `pinned_server_cert_pem` for `https://` targets
- optional HTTP bearer auth shared secret for daemon requests
- optional per-target daemon timeout policy under `[targets.<name>.timeouts]`
- separate startup probe timeout so slow or wedged daemons do not serialize broker startup
- expected daemon target name
- `allow_insecure_http = true` when a target intentionally uses `http://`
- optional `[local]` broker-host config with default working directory, login-shell policy, PTY mode, default shell, embedded-local yield-time policy overrides, and embedded-local `apply_patch` encoding autodetect flag

MCP transport config covers:

- `stdio` by default when `[mcp]` is omitted
- `streamable_http` with a listen address, path, optional stateful-session mode, and optional SSE timing overrides

## Observability

All three runtime components emit diagnostics to `stderr`.

- `remote-exec-broker` keeps `stdout` reserved for the MCP stdio transport, so turning logging up does not corrupt the JSON line protocol.
- Rust components read `REMOTE_EXEC_LOG` first and fall back to `RUST_LOG`.
- `remote-exec-daemon-cpp` also reads `REMOTE_EXEC_LOG` first, then `RUST_LOG`. It accepts a bare level such as `info` or `debug`, and it also understands shared filter strings by honoring `remote_exec_daemon_cpp=<level>`. The previous `remote_exec_daemon_xp=<level>` filter remains accepted as a compatibility alias.
- Default logging is conservative for dependencies and `info` for the project crates.

Examples:

```bash
REMOTE_EXEC_LOG=debug cargo run -p remote-exec-daemon -- configs/daemon.example.toml
REMOTE_EXEC_LOG=debug cargo run -p remote-exec-broker -- configs/broker.example.toml
```

One shared filter string can drive all components:

```bash
REMOTE_EXEC_LOG='warn,remote_exec_broker=debug,remote_exec_daemon=debug,remote_exec_daemon_cpp=debug'
```

## TLS / CA setup

Rust broker and daemon targets use mutual TLS by default:

- the broker's `broker-tls` Cargo feature is enabled by default
- the Rust daemon's `tls` Cargo feature is enabled by default
- the daemon presents a server certificate signed by your CA
- the broker presents a client certificate signed by the same CA
- both sides trust the CA certificate configured in `ca_pem`

If you build `remote-exec-broker` without its default `broker-tls` feature, it rejects `https://` daemon targets and `https://` broker URLs. Stdio and plain `http://` endpoints remain available.

If you build `remote-exec-daemon` without its default `tls` feature, it only supports `transport = "http"` and rejects `transport = "tls"` at startup.

If you explicitly configure a Rust daemon with `transport = "http"`, build it without the `tls` feature, or target `remote-exec-daemon-cpp`, the broker target must use `http://...` together with `allow_insecure_http = true`.

Optional `http_auth` / `http_auth_bearer_token` bearer auth can add request authentication for plain-HTTP daemon links, but it does not add confidentiality or integrity protection. Use TLS when you need transport security.

Preferred bootstrap flow:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs \
  --target builder-a \
  --target builder-b
```

Reuse an existing CA from a previous `dev-init` bundle:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs-next \
  --target builder-c \
  --reuse-ca-from-dir ./remote-exec-certs
```

Reuse an existing CA from explicit PEM paths:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs-next \
  --target builder-c \
  --reuse-ca-cert-pem ./remote-exec-ca/ca.pem \
  --reuse-ca-key-pem ./remote-exec-ca/ca.key
```

Add explicit daemon SANs when the broker will connect by DNS name or non-localhost IP:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs \
  --target builder-a \
  --san builder-a=dns:builder-a.example.com \
  --san builder-a=ip:10.0.0.12
```

This command writes:

- `ca.pem` and `ca.key`
- `broker.pem` and `broker.key`
- `daemons/<target>.pem` and `daemons/<target>.key` for each target
- `certs-manifest.json`

Lower-level certificate commands are also available when you do not want a full bundle:

Generate only a CA:

```bash
cargo run -p remote-exec-admin -- certs init-ca \
  --out-dir ./remote-exec-ca
```

Issue only a broker certificate from an existing CA:

```bash
cargo run -p remote-exec-admin -- certs issue-broker \
  --ca-cert-pem ./remote-exec-ca/ca.pem \
  --ca-key-pem ./remote-exec-ca/ca.key \
  --out-dir ./remote-exec-broker-cert
```

Issue one daemon certificate from an existing CA:

```bash
cargo run -p remote-exec-admin -- certs issue-daemon \
  --ca-cert-pem ./remote-exec-ca/ca.pem \
  --ca-key-pem ./remote-exec-ca/ca.key \
  --out-dir ./remote-exec-daemon-cert \
  --target builder-a \
  --san dns:builder-a.example.com \
  --san ip:10.0.0.12
```

Notes:

- If a target has no `--san` entries, `remote-exec-admin` defaults that daemon cert to `DNS:localhost` and `IP:127.0.0.1`. `certs dev-init` still accepts the old `--daemon-san` spelling as an alias.
- The command prints broker and daemon config snippets after generation so you can paste the generated file paths directly into `configs/broker.example.toml` and `configs/daemon.example.toml`.
- Keep `expected_daemon_name` set to the daemon's configured `target`; it is the application-level identity check on top of transport security.
- Generated private-key files are restricted when written: Unix uses `0600` permissions, and Windows writes a protected file DACL granting access only to the current user, local Administrators, and LocalSystem.
- `skip_server_name_verification = true` keeps CA, key-usage, and expiry validation but skips matching the broker URL host against the daemon certificate SANs.
- `pinned_server_cert_pem` adds an exact daemon leaf-certificate pin on top of CA validation. The PEM file may contain multiple acceptable leaf certificates to ease certificate rotation.
- `tls.pinned_client_cert_pem` adds an exact broker leaf-certificate pin on top of the daemon's normal client-certificate CA validation. The PEM file may contain multiple acceptable broker leaf certificates to ease rotation.
- Re-run with `--force` if you want to overwrite an existing output directory.
- `certs dev-init` is the only command that writes `certs-manifest.json`; the standalone issuance commands write only the PEM files they are responsible for.

Manual `openssl` flow remains available as a fallback:

Minimum files:

- `ca.pem` and `ca.key`
- `broker.pem` and `broker.key`
- one `daemon.pem` and `daemon.key` pair per daemon

Example `openssl` flow:

```bash
# 1) Create a CA
openssl genrsa -out ca.key 4096
openssl req -x509 -new -key ca.key -sha256 -days 3650 \
  -out ca.pem -subj "/CN=remote-exec-ca"

# 2) Create the broker client certificate
openssl genrsa -out broker.key 4096
openssl req -new -key broker.key -out broker.csr \
  -subj "/CN=remote-exec-broker"
cat > broker.ext <<'EOF'
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=clientAuth
EOF
openssl x509 -req -in broker.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
  -out broker.pem -days 825 -sha256 -extfile broker.ext

# 3) Create a daemon server certificate
openssl genrsa -out daemon.key 4096
openssl req -new -key daemon.key -out daemon.csr \
  -subj "/CN=builder-a.example.com"
cat > daemon.ext <<'EOF'
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:builder-a.example.com,IP:127.0.0.1
EOF
openssl x509 -req -in daemon.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
  -out daemon.pem -days 825 -sha256 -extfile daemon.ext
```

Notes:

- Generate a distinct daemon certificate for each host and set its `subjectAltName` to match the hostname or IP used in the broker `base_url`, unless that broker target intentionally sets `skip_server_name_verification = true`.
- Reuse the same broker client certificate for multiple targets if you want, as long as every daemon trusts the same CA.
- Keep `ca.key` private and distribute `ca.pem` to the broker and daemons.
- Set `tls.pinned_client_cert_pem` on a daemon if you want it to accept only one or more exact broker leaf certificates in addition to normal CA-based client-auth checks.

Wire those files into the example configs:

- broker targets use `ca_pem`, `client_cert_pem`, `client_key_pem`, `expected_daemon_name`, and optionally `skip_server_name_verification` / `pinned_server_cert_pem` as shown in `configs/broker.example.toml`
- broker targets can also set `[targets.<name>.http_auth] bearer_token = "..."` when the daemon expects `Authorization: Bearer ...`
- each TLS-enabled daemon built with the default `tls` feature uses `tls.cert_pem`, `tls.key_pem`, `tls.ca_pem`, and optionally `tls.pinned_client_cert_pem` as shown in `configs/daemon.example.toml`
- Rust daemons can also set `[http_auth] bearer_token = "..."`, and the C++ daemon can set `http_auth_bearer_token = ...`
- set `transport = "http"` on a Rust daemon if you intentionally want plain HTTP instead of mutual TLS, or when you build without the `tls` feature
- set `experimental_apply_patch_target_encoding_autodetect = true` on a daemon if you want experimental `apply_patch` support for existing non-UTF-8 text files
- set `expected_daemon_name` to the daemon's configured `target`

Example plain-HTTP target in broker config:

```toml
[targets.builder-cpp]
base_url = "http://builder-cpp.example.com:8181"
allow_insecure_http = true
expected_daemon_name = "builder-cpp"

[targets.builder-cpp.http_auth]
bearer_token = "shared-secret"
```

Example plain-HTTP Rust daemon config:

```toml
target = "builder-a"
listen = "0.0.0.0:8181"
default_workdir = "/srv/work"
transport = "http"

[http_auth]
bearer_token = "shared-secret"
```

`default_workdir` must already exist when the daemon starts.

Example daemon-side broker pin:

```toml
target = "builder-a"
listen = "0.0.0.0:9443"
default_workdir = "/srv/work"

[tls]
cert_pem = "/etc/remote-exec/daemon.pem"
key_pem = "/etc/remote-exec/daemon.key"
ca_pem = "/etc/remote-exec/ca.pem"
pinned_client_cert_pem = "/etc/remote-exec/pins/broker.pem"
```

Optional broker-host local target in broker config:

```toml
[local]
default_workdir = "/srv/local-work"
allow_login_shell = true
# pty = "none"
# default_shell = "/bin/sh"
# windows_posix_root = "C:\\msys64"
#
## Optional. Per-operation yield-time policy overrides for the embedded local target.
## [local.yield_time.exec_command]
## default_ms = 10000
## max_ms = 30000
## min_ms = 250
##
## [local.yield_time.write_stdin_poll]
## default_ms = 5000
## max_ms = 300000
## min_ms = 5000
##
## [local.yield_time.write_stdin_input]
## default_ms = 250
## max_ms = 30000
## min_ms = 250
# experimental_apply_patch_target_encoding_autodetect = true
```

`default_workdir` must already exist when the broker starts with `[local]` enabled.

## Local development

The Rust workspace MSRV is `1.85.0`, which is the first stable release with Rust 2024 edition support.

Run the full workspace checks:

```bash
cargo test --workspace
cargo fmt --all --check
```

## Reliability Notes

- The broker starts even if some configured targets are temporarily unreachable.
- Remote target startup probes run concurrently and are bounded by each target's `timeouts.startup_probe_ms`; targets that are unavailable at broker startup are verified before the first forwarded call.
- `transfer_files` uses broker-mediated copy for `local -> remote`, `remote -> local`, `remote -> remote`, and `local -> local`.
- `forward_ports` uses broker-mediated TCP/UDP forwarding between a `listen_side` and `connect_side`; either side may be a configured target or `"local"`.
- Public `session_id` and `forward_id` values are broker-owned in-memory runtime state. A broker restart drops those mappings, so live exec sessions and port forwards must be reopened after restart.
- `forward_ports` v4 uses `X-Remote-Exec-Port-Tunnel-Version: 4` for daemon-private HTTP/1.1 Upgrade tunnels. Header matching is case-insensitive, but the protocol version is `4`.
- v4 frame numbers 20 and 21 are reserved as `ForwardRecovering` and `ForwardRecovered`; current implementations report recovery state through broker-owned `forward_ports list` fields instead of daemon-emitted recovery frames.
- `forward_ports list` includes `phase`, per-side health, generation, reconnect counters, dropped TCP stream counters, dropped UDP datagram counters, and effective limits. Drop counters include broker-observed drops plus daemon-reported recoverable local drops, such as TCP accepts or UDP datagrams rejected under daemon-side forwarding pressure. The `limits` object is the per-forward ceiling after the broker clamps broker config by both listen-side and connect-side daemon `TunnelReady.limits` values. Pending TCP byte budgets and reconnecting-forward budgets are broker-owned.
- Treat `phase` as the precise live state. Legacy `status = "open"` means the forward has not been explicitly closed or terminally failed, while `phase = "reconnecting"` means at least one side is still recovering and new work may fail until both sides are ready.
- `forward_ports` survives transient broker-daemon transport disconnects when the daemon stays alive. The broker actively heartbeats each upgraded tunnel and may recover from transport loss or missed heartbeat acknowledgements on either forwarding side. The daemon retains the forward itself plus future TCP accepts or future UDP datagrams on the listen side, but active TCP streams and UDP per-peer connector state are not preserved across reconnect.
- Active TCP streams are closed across listen-side or connect-side tunnel reconnect. UDP per-peer connector state is recreated after reconnect, and datagrams that cannot be relayed under pressure are counted as drops.
- Broker and daemon forwarding limits protect open forwards, reconnecting forwards, retained sessions, retained listeners, active TCP streams, UDP peers/binds, queued tunnel bytes, and outbound TCP connect attempts. The broker uses the minimum advertised active TCP stream, UDP peer/bind, and queued-byte limits from the two daemon tunnels when reporting and enforcing a forward. The Rust daemon enforces tunnel connection, retained session/listener, UDP bind, active TCP stream, outbound queued-frame byte, and TCP connect timeout limits. The C++ daemon enforces its v4 retained session/listener, UDP bind, active TCP stream, outbound queued-frame byte, worker-thread, tunnel I/O timeout, and TCP connect timeout limits; both daemons count active TCP streams after connect succeeds rather than while a connect attempt is pending.
- Per-stream TCP connect failures close only that accepted TCP stream; the parent forward remains open for later connections.
- Remote daemon port-forward resources are coordinated through daemon-private HTTP/1.1 Upgrade tunnels. If the broker disappears without closing a port forward, the daemon keeps detached listen-side resources only until the reconnect grace window expires, then reclaims listeners and UDP sockets so the same endpoint can be rebound later.
- A daemon restart still destroys the forward. Unexpected broker loss may therefore delay remote listener cleanup until the reconnect grace window expires, but reopening after broker or daemon restart still creates a new `forward_id`.
- Rust daemon shutdown now cancels pending tunnel accept/read/write work promptly and closes live forwarded listeners, UDP sockets, and TCP connections before the daemon exits.
- Recoverable peer aborts or resets during C++ daemon forwarding are surfaced as normal forward errors and do not terminate the daemon process.
- Internal transfer transport uses GNU tar for both files and directories. Single-file transfers use one fixed archive entry named `.remote-exec-file`. Non-fatal export warnings are carried in a reserved `.remote-exec-transfer-summary.json` archive entry that import consumes and never writes to the destination.
- `transfer_files` accepts either a single `source` or a `sources` array. The optional `destination_mode` defaults to `auto`: single-source transfers behave like `cp` by copying under `destination.path` when it is an existing directory or ends in a path separator, otherwise using it as the exact final path; multi-source transfers use it as a directory root and place each source under its basename. Use `destination_mode = "into_directory"` to copy under `destination.path` by basename, or `destination_mode = "exact"` to force exact-path semantics.
- `transfer_files` also accepts an optional `exclude` array of glob patterns that is applied independently to each source root during export. Matching uses `/` as the logical separator on every platform and supports `*`, `?`, `**`, `[abc]`, `[a-z]`, `[!abc]`, `[!a-c]`, `[^abc]`, and `[^a-c]`.
- `transfer_files` `exclude` silently omits matching descendants from exported directory trees. Excluded directories are pruned recursively. In v1, single-file sources ignore `exclude`, and the top-level source path itself is never excluded.
- `transfer_files` `overwrite` defaults to `merge`. `fail` rejects any existing destination, `merge` overlays files into an existing compatible file or directory without deleting unrelated directory entries, and `replace` removes the existing destination before importing.
- `transfer_files` skips unsupported source entries inside directory trees such as device nodes, FIFOs, sockets, and other special files, and reports structured/text warnings.
- `transfer_files` `symlink_mode` defaults to `preserve`, which archives symlinks as symlinks instead of following them. Other modes are `follow` to copy symlink targets and `skip` to omit symlinks with warnings. Windows XP-compatible `remote-exec-daemon-cpp` builds skip symlink entries inside directory transfers and import archives when preservation is unavailable; `follow` follows regular-file and directory symlink targets when the platform exposes them.
- `transfer_files` does not expose a public compression option. The broker automatically uses `zstd` for internal transfer staging only when its own config and every participating remote daemon support it, and otherwise falls back to uncompressed staging.
- When structured content is enabled, `transfer_files` structured results always include `sources`; the legacy `source` field is only populated for single-source transfers. Non-fatal skips are returned in `warnings` and are also prepended to the text output.
- Broker and daemon configs each support `enable_transfer_compression = false` to force internal transfer staging to stay uncompressed.
- Broker `[local]` config enables `target: "local"` for `exec_command`, `write_stdin`, `apply_patch`, and `view_image` on the broker host.
- `transfer_files` structured results include both the requested `destination` and the broker-computed `resolved_destination`.
- `write_stdin` normalizes lost sessions into the usual unknown-process error when the broker no longer has the mapping or when the daemon restarted or explicitly reports `unknown_session`.
- `max_output_tokens` is enforced by the daemon for command output as an approximate budget where one token is about four UTF-8 bytes. Both daemons compute `original_token_count` as `ceil(total_utf8_bytes / 4)`.
- When command output is truncated, both daemons prepend `Total output lines: N\n\n` and render a UTF-8-safe roughly 50/50 head/tail split around the marker `…X tokens truncated…`. Omitted `max_output_tokens` still defaults to `10000`, while explicit `0` still returns an empty `output`.
- Non-TTY `exec_command` output on both the main daemon and `remote-exec-daemon-cpp` merges `stdout` and `stderr` through one pipe so the single public `output` field preserves their emitted order.
- Daemon config can override `yield_time_ms` policy separately for `exec_command`, empty `write_stdin` polls, and non-empty `write_stdin` writes. Each bucket supports `default_ms`, `max_ms`, and `min_ms`, where `min_ms` silently raises smaller caller-provided values.
- Broker `[local]` supports the same nested `yield_time` config for the embedded broker-host local target. `remote-exec-daemon-cpp` supports the same three buckets with flat `yield_time_*` INI keys.
- Each target daemon keeps at most `64` live exec sessions. When full, it protects the `8` most recently touched sessions, prunes exited sessions first, otherwise prunes the oldest non-protected live session, and terminates the pruned process.
- `apply_patch` supports the documented `*** End of File` marker.
- `apply_patch` preserves an updated file's existing `LF` versus `CRLF` line ending style.
- `apply_patch` is intentionally non-transactional across multiple file actions. Actions are applied in order; if a later action fails, earlier successful file changes are left on disk and the returned error describes the failure point. Callers that need all-or-nothing behavior should split work into smaller patches or verify state before and after applying.
- Daemons can opt into experimental `experimental_apply_patch_target_encoding_autodetect = true` support so `apply_patch` can read and rewrite existing non-UTF-8 text files using the autodetected original encoding. The current test coverage explicitly includes UTF-16LE plus common East Asian encodings such as Shift_JIS, GBK, Big5, and EUC-KR.
- Broker `[local]` config can also set `experimental_apply_patch_target_encoding_autodetect = true` to enable the same behavior for the embedded broker-host local target only.
- Successful `apply_patch` calls return text output only; they do not expose MCP `structuredContent`.
- `exec_command` intercepted into `apply_patch` always returns a warning in structured content `warnings` when structured content is enabled, and in normal text output either way.
- `exec_command` returns a warning in structured content `warnings` when structured content is enabled, and in normal text output when a target crosses from `59` to `60` open exec sessions.
- Broker config supports `disable_structured_content = true` to omit MCP `structuredContent` from successful tool responses.
- `transfer_files` normalizes Windows path separators before filesystem access on Windows endpoints.
- `transfer_files` compares Windows paths case-insensitively when checking obvious same-path collisions.
- `forward_ports` accepts `action = "open" | "list" | "close"`; `open` requires `listen_side`, `connect_side`, and one or more `forwards`, `list` can filter by side or `forward_ids`, and `close` requires explicit `forward_ids`.
- `forward_ports` endpoint strings accept a bare port such as `"8080"` as shorthand for `"127.0.0.1:8080"`. Non-loopback bind addresses such as `"0.0.0.0:8080"` are allowed. `listen_endpoint` may use port `0`; `connect_endpoint` must use a nonzero port.
- Failed `forward_ports` initialization is all-or-nothing: any listeners created by the failed call are closed and no failed forward remains listed.
- Explicit `forward_ports` close reports an error if daemon-side listener cleanup cannot be confirmed; failed forwards remain listed so callers can retry or inspect them.
- Executable preservation is best effort and only restored on platforms that expose executable mode bits.
- `allow_login_shell` controls daemon login-shell policy and defaults to `true`; explicit `login=true` is rejected only when the daemon disables it.
- `default_shell` lets the daemon pin its fallback shell on both Unix and Windows. Startup now fails if the configured shell, or the auto-detected fallback when `default_shell` is omitted, is not usable on that host. Set this to `powershell.exe` or `cmd.exe` on Windows if you do not want the new Git Bash-first default.
- On Windows, `windows_posix_root` lets the daemon treat single-slash POSIX paths such as `/usr/bin/bash` or `/tmp/work` as absolute paths rooted under a configured Cygwin/MSYS install or workspace directory. This applies to `workdir`, image paths, patch file paths, transfer endpoints, and configured shell paths. Raw command strings are not rewritten.
- Broker `[local]` config supports the same `windows_posix_root` setting for the embedded broker-host local target.
- On Windows, `login=false` suppresses shell startup state where supported: Git Bash omits `-l`, `pwsh` and `powershell` add `-NoProfile`, and `cmd.exe` adds `/D` to disable AutoRun. `login=true` uses Git Bash with `-l -c` and drops those PowerShell and `cmd.exe` suppression flags. When the selected Windows bash comes from `windows_posix_root`, the daemon also sets `CHERE_INVOKING=1` so login shells preserve the requested cwd.
- On Windows, tool path inputs also accept MSYS/Cygwin drive-style absolute paths such as `/c/work/file.txt` and `/cygdrive/c/work/file.txt` even without `windows_posix_root`.
- `list_targets` reports the daemon's actual `supports_pty` capability instead of assuming PTY support.
- The `remote-exec-broker` Cargo feature `broker-tls` is enabled by default. Builds that disable it reject `https://` daemon targets and `https://` broker URLs, but still support stdio and plain `http://` streamable HTTP.
- The `remote-exec-daemon` Cargo feature `tls` is enabled by default. Builds that disable it no longer accept `transport = "tls"` and must use `transport = "http"` instead.
- `pty = "none"` disables TTY entirely. On Windows, `pty = "conpty"` or `pty = "winpty"` force that backend and startup fails if the selected backend is unavailable. The `remote-exec-daemon` Cargo feature `winpty` is enabled by default, and `remote-exec-broker` forwards it for the embedded local target. Builds that disable that feature no longer expose the `winpty` backend. When `pty` is omitted, the daemon keeps the current auto-detect behavior.
- Windows PTY output is passed through a streaming VT parser before it is returned, so control-only traffic such as terminal queries, OSC title updates, cursor visibility toggles, color resets, and erase-line controls is stripped while printable text and line endings are preserved.
- Child process environments are normalized for deterministic non-interactive output. On Windows this now also includes `LANG`, `LC_CTYPE`, and `LC_ALL`, all forced to `C.UTF-8`; on Unix the daemon picks the best available UTF-8 locale and falls back to `LANG=C` when needed.
- `winptyrs` now prefers static linking when both static and dynamic layouts are available. Set `WINPTY_STATIC=0` to force dynamic linking instead.
- Default shell resolution uses `default_shell` when configured. Otherwise it tries `SHELL`, then a usable passwd shell, then `bash`, then `/bin/sh` on Unix; and a bash under `windows_posix_root` when configured, then Git Bash, then `pwsh.exe`, then `powershell.exe` or `powershell`, then `COMSPEC`, then `cmd.exe` on Windows.
- Git Bash auto-discovery on Windows checks `windows_posix_root` first when configured, then standard Git for Windows install roots and locations derivable from `git.exe` on `PATH`. Portable or unusual installs should set `default_shell` or `windows_posix_root` explicitly.
- `remote-exec-daemon-cpp` is intentionally narrower than the main daemon: POSIX builds support `tty=true` when PTY allocation is available, Windows XP-compatible builds reject `tty=true`, `view_image` supports passthrough reads for PNG, JPEG, and WebP only and defaults omitted `detail` to `original`, TLS is unavailable, static path sandboxing is available for exec cwd, transfer read/write endpoints, patch write targets, and `view_image` reads, regular-file transfers, directory trees, and broker-built multi-source transfer bundles are supported, transfer import/export bodies stream through the daemon without staging a full tar archive in memory, and transfer staging always falls back to uncompressed payloads. Its `forward_ports` support uses the v4 tunnel protocol and enforces configured retained-session, retained-listener, UDP-bind, active-TCP-stream, queued-byte, worker-thread, tunnel I/O timeout, and TCP connect timeout limits. Each active C++ forwarded TCP stream uses separate read and write workers, so size `port_forward_max_worker_threads` for full-duplex stream concurrency. POSIX C++ builds can preserve, follow, or skip source symlinks. Windows XP-compatible C++ builds skip symlink entries inside directory transfers and import archives when preservation is unavailable, while `follow` copies regular-file and directory targets when exposed by the platform. On POSIX it follows the Rust daemon's default shell policy and forces `LC_ALL=C.UTF-8` plus `LANG=C.UTF-8`; on Windows XP-compatible builds it supports `cmd.exe`. Hard links, sparse entries, malformed archive paths, and non-passthrough image formats remain unsupported there; special files are skipped during export.

## Quality Gate

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
# From an x86 Visual Studio developer prompt with the v141_xp-capable toolset:
nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-xp
```

Run the broker end-to-end test only:

```bash
cargo test -p remote-exec-broker --test multi_target -- --nocapture
```

Focused transfer commands:

```bash
cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture
cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture
cargo test -p remote-exec-broker --test multi_target -- --nocapture
```

Focused no-default-features commands:

```bash
cargo test -p remote-exec-broker --no-default-features --tests
cargo test -p remote-exec-daemon --no-default-features --tests
cargo clippy -p remote-exec-broker --no-default-features --all-targets -- -D warnings
cargo clippy -p remote-exec-daemon --no-default-features --all-targets -- -D warnings
```

Start a daemon:

```bash
cargo run -p remote-exec-daemon -- configs/daemon.example.toml
```

Start the broker:

```bash
cargo run -p remote-exec-broker -- configs/broker.example.toml
```

Call the broker directly from a config file:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  list-targets
```

When `--broker-config` is used, the CLI loads the broker config and invokes the same broker tool handlers in-process. It does not start the broker MCP server, and the config's `[mcp]` transport only matters when running `remote-exec-broker` itself.

Run a command through the broker:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  exec --target builder-a "uname -a"
```

Transfer endpoints use `<target>:<absolute-path>`. Repeat `--source` to transfer multiple inputs.

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-config configs/broker.example.toml \
  transfer-files \
  --source local:/tmp/source.txt \
  --destination builder-a:/tmp/dest.txt
```

Use `apply-patch --input-file -` and `write-stdin --chars-file -` to read input from stdin.

Open a local-to-remote TCP port forward from the CLI against a running broker:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-url http://127.0.0.1:8787/mcp \
  forward-ports open \
  --listen-side local \
  --connect-side builder-a \
  --forward tcp:127.0.0.1:15432=127.0.0.1:5432
```

`forward-ports` state lives in the long-running broker process. In `--broker-config` mode, `remote-exec` rebuilds broker state for that one invocation, so forwards do not persist across separate CLI runs there. Use `phase`, not just legacy `status`, when deciding whether a listed forward is ready: `status = "open"` can coexist with `phase = "reconnecting"` while one side is still recovering. If the broker loses only the daemon transport while the daemon stays alive, including through missed tunnel heartbeat acknowledgements, the broker can reconnect the forward after transport loss on either forwarding side and preserve future listen-side traffic, but active TCP streams and UDP per-peer connector state are still lost. Per-stream TCP connect failures close only that accepted TCP stream and leave the parent forward open. If the broker dies without reconnecting, daemon-side listeners are reclaimed after the reconnect grace window expires, and reopening still creates a new `forward_id`.

Expose the broker over streamable HTTP instead of stdio:

```toml
[mcp]
transport = "streamable_http"
listen = "127.0.0.1:8787"
path = "/mcp"
```

Then connect with the CLI over HTTP:

```bash
cargo run -p remote-exec-broker --bin remote-exec -- \
  --broker-url http://127.0.0.1:8787/mcp \
  list-targets
```

## Trust model

Selecting a target is equivalent to `danger-full-access` on that machine unless static sandbox config restricts the relevant path-based operation.

Selecting `target: "local"` in `transfer_files` uses broker-host filesystem access and is governed by optional broker `host_sandbox` config.

When broker `[local]` config is enabled, selecting `target: "local"` in `exec_command`, `write_stdin`, `apply_patch`, or `view_image` uses the broker host and the same optional broker `host_sandbox` rules.

Selecting side `"local"` in `forward_ports` uses broker-host network access regardless of broker `[local]` exec configuration. `forward_ports` network access is controlled by target selection and static forwarding limits, not filesystem sandbox rules.

In v1:

- there is no sandbox selection flow
- there is no per-call approval flow
- sandbox rules are static config allow/deny lists
- missing `allow` or `allow = []` means allow all, then `deny` refines the allowed set
- `exec_command` only checks the resolved starting `cwd`; it does not inspect arbitrary paths embedded in the command text
- `view_image` checks the resolved final image path for read access
- `apply_patch` checks resolved write targets; its `workdir` is not sandboxed separately
- `transfer_files` checks the source path for read access and the destination path for write access on the respective host
- `forward_ports` can bind non-loopback addresses and connect to arbitrary endpoints reachable from each side

Security is based on target selection plus broker-to-daemon mutual TLS for normal targets, with an explicit insecure-HTTP opt-in only for XP-style targets, not on per-call approval flows.
Configured remote targets may not be named `local`.

## Current status

- Core remote tools are implemented: `list_targets`, `exec_command`, `write_stdin`, `apply_patch`, `view_image`, `transfer_files`, and `forward_ports`.
- The broker now supports MCP stdio and streamable HTTP transports.
- A companion `remote-exec` CLI client can call the broker over stdio or streamable HTTP.
- The broker can optionally expose its own host as `target: "local"` for daemon-backed exec, stdin polling, patch, and image workflows.
- Static path-based sandboxing is available for exec `cwd`, reads, and writes on both daemons and broker-host local access paths.
- Broker and daemon session handling are hardened for concurrent exec workloads and precise restart/session-loss behavior.
- Patch application supports strict EOF-marker handling and repeated-context multi-hunk updates.
- Broker target discovery returns cached daemon metadata when the broker currently considers it usable; otherwise `daemon_info` is `null`.
- The current workspace quality gate passes:
  - `cargo test --workspace`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- CI also exercises broker and daemon `--no-default-features` test and clippy jobs on Ubuntu so the `tls-disabled` code paths stay intentionally covered.
- Linux broker/daemon support plus Windows broker-host and Windows daemon support
- Per-machine daemon deployment
- Static broker target configuration
- No exec-session or port-forward persistence across broker or daemon restart

## Acknowledgments

- The tool surface and behavioral model are heavily influenced by [Codex](https://github.com/openai/codex).
- This project reinterprets those ideas for a remote-first MCP broker plus per-machine daemon architecture on Linux and Windows.

## References

- `AGENTS.md`
- `skills/using-remote-exec-mcp/SKILL.md`
- `configs/broker.example.toml`
- `configs/daemon.example.toml`
