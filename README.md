# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux machines.

## Components

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

## Local development

Run the full workspace checks:

```bash
cargo test --workspace
cargo fmt --all --check
```

## Reliability Notes

- The broker now starts even if some configured targets are temporarily unreachable.
- `write_stdin` only invalidates sessions when the daemon restarted or explicitly reports `unknown_session`.
- `max_output_tokens` is enforced by the daemon for command output.

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

## Current scope

- Linux only
- Per-machine daemon deployment
- Static broker target configuration
- No session persistence across broker or daemon restart

## References

- `docs/local-system-tools.md`
- `docs/specs/2026-03-31-remote-exec-mcp-design.md`
- `docs/superpowers/plans/2026-03-31-remote-exec-mcp.md`
