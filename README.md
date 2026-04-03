# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux and Windows machines.

The tool interfaces and behavior in this project are heavily influenced by [Codex](https://github.com/openai/codex), while the implementation here is a separate remote-first broker and per-machine daemon design.

## Components

- `remote-exec-admin`
  - Administrative CLI for TLS bootstrap and future operator workflows.
- `remote-exec-broker`
  - Public MCP server over stdio.
  - Accepts tool calls with a required `target` for machine-local operations.
  - Owns opaque public `session_id` values for live command sessions.
- `remote-exec-daemon`
  - Per-machine daemon over mTLS JSON/HTTP.
  - Executes commands, manages local sessions, applies patches, and reads images.
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
- `list_targets` is broker-local and does not probe daemons at read time.
- The broker validates `target`, forwards the request to the selected daemon, and returns MCP-compatible content plus structured JSON.
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
- TLS certificate, key, and CA paths

Broker config covers one entry per target:

- daemon base URL
- CA path
- client certificate path
- client key path
- expected daemon target name

## TLS / CA setup

The broker and daemon use mutual TLS:

- the daemon presents a server certificate signed by your CA
- the broker presents a client certificate signed by the same CA
- both sides trust the CA certificate configured in `ca_pem`

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
- `transfer_files` treats `destination.path` as the exact final path to create or replace; it does not infer basenames or copy "into" an existing directory.
- `write_stdin` only invalidates sessions when the daemon restarted or explicitly reports `unknown_session`.
- `max_output_tokens` is enforced by the daemon for command output.
- Each target daemon keeps at most `64` live exec sessions. When full, it protects the `8` most recently touched sessions, prunes exited sessions first, otherwise prunes the oldest non-protected live session, and terminates the pruned process.
- `apply_patch` supports the documented `*** End of File` marker.
- `exec_command` intercepted into `apply_patch` always emits a warning in MCP `_meta.warnings` telling the client to use `apply_patch` directly.
- `exec_command` emits a warning in MCP `_meta.warnings` when a target crosses from `59` to `60` open exec sessions. Warnings stay out of the normal text output.
- `transfer_files` normalizes Windows path separators before filesystem access on Windows endpoints.
- `transfer_files` compares Windows paths case-insensitively when checking obvious same-path collisions.
- Executable preservation is best effort and only restored on platforms that expose executable mode bits.
- `allow_login_shell` controls daemon login-shell policy and defaults to `true`; explicit `login=true` is rejected only when the daemon disables it.
- On Windows, `login=false` suppresses shell startup state where supported: `pwsh` and `powershell` add `-NoProfile`, while `cmd.exe` adds `/D` to disable AutoRun. `login=true` drops those suppression flags.
- `list_targets` reports the daemon's actual `supports_pty` capability instead of assuming PTY support.
- On Windows, `tty=true` prefers the existing ConPTY-backed `portable-pty` path and falls back to `winpty-rs` when ConPTY is unavailable and the native winpty runtime is installed.
- Default shell resolution uses explicit override, then `SHELL`, then a usable passwd shell, then `bash` from `PATH`, then `/bin/sh` on Unix. On Windows it uses explicit override, then the first `pwsh.exe` on `PATH`, then `powershell.exe` or `powershell`, then `COMSPEC`, then `cmd.exe`.

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

Selecting a target is equivalent to `danger-full-access` on that machine.

Selecting `target: "local"` in `transfer_files` is equivalent to full filesystem access on the broker host.

In v1:

- there is no sandbox selection flow
- there is no per-call approval flow
- the daemon can access any file or process available to the daemon user

Security is based on target selection plus broker-to-daemon mutual TLS, not on per-call restrictions.
Configured remote targets may not be named `local`.

## Current status

- Core remote tools are implemented: `list_targets`, `exec_command`, `write_stdin`, `apply_patch`, `view_image`, and `transfer_files`.
- Broker and daemon session handling are hardened for concurrent exec workloads and precise restart/session-loss behavior.
- Patch application supports strict EOF-marker handling and repeated-context multi-hunk updates.
- Broker target discovery returns cached daemon metadata when the broker currently considers it usable; otherwise `daemon_info` is `null`.
- The workspace quality gate is green on `main`:
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

- `docs/local-system-tools.md`
