# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux and Windows machines.

The tool interfaces and behavior in this project are heavily influenced by [Codex](https://github.com/openai/codex), while the implementation here is a separate remote-first broker and per-machine daemon design.

Everything under `docs/` is historical implementation detail and planning context, not the live behavior contract. Treat this `README.md`, `AGENTS.md`, the config examples, and `skills/using-remote-exec-mcp/SKILL.md` as the current source of truth.

## Components

- `remote-exec-admin`
  - Administrative CLI for TLS bootstrap and future operator workflows.
- `remote-exec-broker`
  - Public MCP server over stdio.
  - Accepts tool calls with a required `target` for machine-local operations.
  - Owns opaque public `session_id` values for live command sessions.
  - Can optionally expose the broker host itself as `target: "local"` for daemon-backed `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
  - Always provides broker-host filesystem access for `transfer_files` endpoints that use `target: "local"`, even when the broker `[local]` target is disabled.
- `remote-exec-daemon`
  - Per-machine daemon over mTLS JSON/HTTP.
  - Executes commands, manages local sessions, applies patches, reads images, and serves transfer archives.
- `remote-exec-daemon-xp`
  - Standalone Windows XP daemon over plain HTTP.
  - Supports `exec_command`, `write_stdin`, `apply_patch`, and `transfer_files` for files, directories, and broker-built multi-source bundles.
  - Does not support PTY or image reads.
- `remote-exec-proto`
  - Shared public tool schemas and broker-daemon RPC types.

## Supported tools

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`

## Architecture

- Agents talk only to the broker.
- Agents can call `list_targets` to discover configured logical target names and cached daemon metadata when available.
- When broker `[local]` config is enabled, `list_targets` also includes `local` for the broker host.
- `list_targets` is broker-local and does not probe daemons at read time.
- The broker validates `target`, forwards the request to the selected daemon, and returns MCP-compatible content plus structured JSON.
- For the optional `local` target, the broker reuses daemon execution logic in-process instead of asking operators to run a second same-host daemon manually.
- Each daemon serves exactly one configured target machine.
- Live exec sessions are broker-routed by opaque public `session_id`, not by daemon-local process identifiers.

## Configuration

Example configs live in:

- `configs/broker.example.toml`
- `configs/daemon.example.toml`

Daemon config covers:

- target name
- listen address
- default working directory
- optional static sandbox allow/deny rules for exec `cwd`, reads, and writes
- optional transfer compression support toggle
- optional default shell override
- optional PTY mode selection
- TLS certificate, key, and CA paths

Broker config covers one entry per target:

- optional broker-host sandbox allow/deny rules for exec `cwd`, reads, and writes
- optional broker-side transfer compression support toggle
- daemon base URL
- CA path for `https://` targets
- client certificate path for `https://` targets
- client key path for `https://` targets
- expected daemon target name
- `allow_insecure_http = true` when a target intentionally uses `http://`
- optional `[local]` broker-host config with default working directory, login-shell policy, PTY mode, and default shell

## Observability

All three runtime components emit diagnostics to `stderr`.

- `remote-exec-broker` keeps `stdout` reserved for the MCP stdio transport, so turning logging up does not corrupt the JSON line protocol.
- Rust components read `REMOTE_EXEC_LOG` first and fall back to `RUST_LOG`.
- `remote-exec-daemon-xp` also reads `REMOTE_EXEC_LOG` first, then `RUST_LOG`. It accepts a bare level such as `info` or `debug`, and it also understands shared filter strings by honoring `remote_exec_daemon_xp=<level>`.
- Default logging is conservative for dependencies and `info` for the project crates.

Examples:

```bash
REMOTE_EXEC_LOG=debug cargo run -p remote-exec-daemon -- configs/daemon.example.toml
REMOTE_EXEC_LOG=debug cargo run -p remote-exec-broker -- configs/broker.example.toml
```

One shared filter string can drive all components:

```bash
REMOTE_EXEC_LOG='warn,remote_exec_broker=debug,remote_exec_daemon=debug,remote_exec_daemon_xp=debug'
```

## TLS / CA setup

The broker and daemon use mutual TLS:

- the daemon presents a server certificate signed by your CA
- the broker presents a client certificate signed by the same CA
- both sides trust the CA certificate configured in `ca_pem`

`remote-exec-daemon-xp` is the exception for v1. It uses plain HTTP only, so broker targets that point at it must use `http://...` together with `allow_insecure_http = true`.

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
  --daemon-san builder-a=dns:builder-a.example.com \
  --daemon-san builder-a=ip:10.0.0.12
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

- If a target has no `--daemon-san` entries, `remote-exec-admin` defaults that daemon cert to `DNS:localhost` and `IP:127.0.0.1`.
- The command prints broker and daemon config snippets after generation so you can paste the generated file paths directly into `configs/broker.example.toml` and `configs/daemon.example.toml`.
- Keep `expected_daemon_name` set to the daemon's configured `target`; it is the application-level identity check on top of TLS.
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

- Generate a distinct daemon certificate for each host and set its `subjectAltName` to match the hostname or IP used in the broker `base_url`.
- Reuse the same broker client certificate for multiple targets if you want, as long as every daemon trusts the same CA.
- Keep `ca.key` private and distribute `ca.pem` to the broker and daemons.

Wire those files into the example configs:

- broker targets use `ca_pem`, `client_cert_pem`, `client_key_pem`, and `expected_daemon_name` as shown in `configs/broker.example.toml`
- each daemon uses `tls.cert_pem`, `tls.key_pem`, and `tls.ca_pem` as shown in `configs/daemon.example.toml`
- set `expected_daemon_name` to the daemon's configured `target`

Example XP target in broker config:

```toml
[targets.builder-xp]
base_url = "http://builder-xp.example.com:8181"
allow_insecure_http = true
expected_daemon_name = "builder-xp"
```

Optional broker-host local target in broker config:

```toml
[local]
default_workdir = "/srv/local-work"
allow_login_shell = true
# pty = "none"
# default_shell = "/bin/sh"
```

## Local development

Run the full workspace checks:

```bash
cargo test --workspace
cargo fmt --all --check
```

## Reliability Notes

- The broker now starts even if some configured targets are temporarily unreachable.
- Targets that are unavailable at broker startup are verified before the first forwarded call.
- `transfer_files` uses broker-mediated copy for `local -> remote`, `remote -> local`, `remote -> remote`, and `local -> local`.
- Internal transfer transport uses GNU tar for both files and directories. Single-file transfers use one fixed archive entry named `.remote-exec-file`.
- `transfer_files` accepts either a single `source` or a `sources` array. Multi-source transfers treat `destination.path` as a directory root and place each source under its basename.
- `transfer_files` can optionally compress archive payloads with `zstd` when the broker and every participating daemon allow transfer compression. `compression = "none"` remains the default.
- `transfer_files` structured results always include `sources`; the legacy `source` field is only populated for single-source transfers.
- Broker and daemon configs each support `enable_transfer_compression = false` to reject compressed transfers without disabling uncompressed transfers.
- Broker `[local]` config enables `target: "local"` for `exec_command`, `write_stdin`, `apply_patch`, and `view_image` on the broker host.
- `transfer_files` treats `destination.path` as the exact final path to create or replace for single-source transfers; it does not infer basenames or copy "into" an existing directory in that mode.
- `write_stdin` only invalidates sessions when the daemon restarted or explicitly reports `unknown_session`.
- `max_output_tokens` is enforced by the daemon for command output.
- Each target daemon keeps at most `64` live exec sessions. When full, it protects the `8` most recently touched sessions, prunes exited sessions first, otherwise prunes the oldest non-protected live session, and terminates the pruned process.
- `apply_patch` supports the documented `*** End of File` marker.
- `exec_command` intercepted into `apply_patch` always returns a warning in structured content `warnings` and in normal text output telling the client to use `apply_patch` directly.
- `exec_command` returns a warning in structured content `warnings` and in normal text output when a target crosses from `59` to `60` open exec sessions.
- `transfer_files` normalizes Windows path separators before filesystem access on Windows endpoints.
- `transfer_files` compares Windows paths case-insensitively when checking obvious same-path collisions.
- Executable preservation is best effort and only restored on platforms that expose executable mode bits.
- `allow_login_shell` controls daemon login-shell policy and defaults to `true`; explicit `login=true` is rejected only when the daemon disables it.
- `default_shell` lets the daemon pin its fallback shell on both Unix and Windows. Startup now fails if the configured shell, or the auto-detected fallback when `default_shell` is omitted, is not usable on that host. Set this to `powershell.exe` or `cmd.exe` on Windows if you do not want the new Git Bash-first default.
- On Windows, `login=false` suppresses shell startup state where supported: Git Bash omits `-l`, `pwsh` and `powershell` add `-NoProfile`, and `cmd.exe` adds `/D` to disable AutoRun. `login=true` uses Git Bash with `-l -c` and drops those PowerShell and `cmd.exe` suppression flags.
- On Windows, tool path inputs also accept MSYS/Cygwin drive-style absolute paths such as `/c/work/file.txt` and `/cygdrive/c/work/file.txt` for `workdir`, image paths, patch file paths, and transfer endpoints. Raw command strings are not rewritten.
- `list_targets` reports the daemon's actual `supports_pty` capability instead of assuming PTY support.
- `pty = "none"` disables TTY entirely. On Windows, `pty = "conpty"` or `pty = "winpty"` force that backend and startup fails if the selected backend is unavailable. When `pty` is omitted, the daemon keeps the current auto-detect behavior.
- Default shell resolution uses `default_shell` when configured. Otherwise it tries `SHELL`, then a usable passwd shell, then `bash`, then `/bin/sh` on Unix; and Git Bash, then `pwsh.exe`, then `powershell.exe` or `powershell`, then `COMSPEC`, then `cmd.exe` on Windows.
- Git Bash auto-discovery on Windows only checks standard Git for Windows install roots and locations derivable from `git.exe` on `PATH`. Portable or unusual installs should set `default_shell` to an explicit path.
- `remote-exec-daemon-xp` is intentionally narrower than the main daemon: it always uses `cmd.exe`, rejects `tty=true`, does not implement `view_image`, supports regular-file transfers, directory trees, and broker-built multi-source transfer bundles, and never enables transfer compression. Symlinks, hard links, special files, sparse entries, and malformed archive paths remain unsupported there.

## Quality Gate

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
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

Start a daemon:

```bash
cargo run -p remote-exec-daemon -- configs/daemon.example.toml
```

Start the broker:

```bash
cargo run -p remote-exec-broker -- configs/broker.example.toml
```

## Trust model

Selecting a target is equivalent to `danger-full-access` on that machine unless static sandbox config restricts the relevant path-based operation.

Selecting `target: "local"` in `transfer_files` uses broker-host filesystem access and is governed by optional broker `host_sandbox` config.

When broker `[local]` config is enabled, selecting `target: "local"` in `exec_command`, `write_stdin`, `apply_patch`, or `view_image` uses the broker host and the same optional broker `host_sandbox` rules.

In v1:

- there is no sandbox selection flow
- there is no per-call approval flow
- sandbox rules are static config allow/deny lists
- missing `allow` or `allow = []` means allow all, then `deny` refines the allowed set
- `exec_command` only checks the resolved starting `cwd`; it does not inspect arbitrary paths embedded in the command text
- `view_image` checks the resolved final image path for read access
- `apply_patch` checks resolved write targets; its `workdir` is not sandboxed separately
- `transfer_files` checks the source path for read access and the destination path for write access on the respective host

Security is based on target selection plus broker-to-daemon mutual TLS for normal targets, with an explicit insecure-HTTP opt-in only for XP-style targets, not on per-call approval flows.
Configured remote targets may not be named `local`.

## Current status

- Core remote tools are implemented: `list_targets`, `exec_command`, `write_stdin`, `apply_patch`, `view_image`, and `transfer_files`.
- The broker can optionally expose its own host as `target: "local"` for daemon-backed exec, stdin polling, patch, and image workflows.
- Static path-based sandboxing is available for exec `cwd`, reads, and writes on both daemons and broker-host local access paths.
- Broker and daemon session handling are hardened for concurrent exec workloads and precise restart/session-loss behavior.
- Patch application supports strict EOF-marker handling and repeated-context multi-hunk updates.
- Broker target discovery returns cached daemon metadata when the broker currently considers it usable; otherwise `daemon_info` is `null`.
- The current workspace quality gate passes:
  - `cargo test --workspace`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Linux broker/daemon support plus Windows broker-host and Windows daemon support
- Per-machine daemon deployment
- Static broker target configuration
- No session persistence across broker or daemon restart

## Acknowledgments

- The tool surface and behavioral model are heavily influenced by [Codex](https://github.com/openai/codex).
- This project reinterprets those ideas for a remote-first MCP broker plus per-machine daemon architecture on Linux and Windows.

## References

- `AGENTS.md`
- `skills/using-remote-exec-mcp/SKILL.md`
- `configs/broker.example.toml`
- `configs/daemon.example.toml`
