# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux machines.

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

- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

## Architecture

- Agents talk only to the broker.
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

Notes:

- If a target has no `--daemon-san` entries, `remote-exec-admin` defaults that daemon cert to `DNS:localhost` and `IP:127.0.0.1`.
- The command prints broker and daemon config snippets after generation so you can paste the generated file paths directly into `configs/broker.example.toml` and `configs/daemon.example.toml`.
- Keep `expected_daemon_name` set to the daemon's configured `target`; it is the application-level identity check on top of TLS.
- Re-run with `--force` if you want to overwrite an existing output directory.

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
- `write_stdin` only invalidates sessions when the daemon restarted or explicitly reports `unknown_session`.
- `max_output_tokens` is enforced by the daemon for command output.
- `apply_patch` supports the documented `*** End of File` marker.
- Default shell resolution uses explicit override, then `SHELL`, then a usable passwd shell, then `bash` from `PATH`, then `/bin/sh`.

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

In v1:

- there is no sandbox selection flow
- there is no per-call approval flow
- the daemon can access any file or process available to the daemon user

Security is based on target selection plus broker-to-daemon mutual TLS, not on per-call restrictions.

## Current status

- Core remote tools are implemented: `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
- Broker and daemon session handling are hardened for concurrent exec workloads and precise restart/session-loss behavior.
- Patch application supports strict EOF-marker handling and repeated-context multi-hunk updates.
- The workspace quality gate is green on `main`:
  - `cargo test --workspace`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Linux only
- Per-machine daemon deployment
- Static broker target configuration
- No session persistence across broker or daemon restart

## Acknowledgments

- The tool surface and behavioral model are heavily influenced by [Codex](https://github.com/openai/codex).
- This project reinterprets those ideas for a remote-first MCP broker plus per-machine daemon architecture on Linux.

## References

- `docs/local-system-tools.md`
