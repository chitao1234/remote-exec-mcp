# AGENTS.md

## Scope

These instructions apply to the entire `remote-exec-mcp` workspace.

## Project Overview

- This repository is a Rust 2024 workspace for a remote-first MCP server that exposes Codex-style local system tools across multiple Linux and Windows machines, plus a narrower standalone Windows XP daemon.
- The public tool surface is currently `list_targets`, `exec_command`, `write_stdin`, `apply_patch`, `view_image`, and `transfer_files`.
- The architecture is intentionally split:
  - `remote-exec-broker` is the public MCP server over stdio. It validates `target`, routes requests to daemons, and owns the opaque public `session_id` namespace.
  - `remote-exec-daemon` is the per-machine mTLS JSON/HTTP server that performs local execution, patching, image reads, transfer archive import/export, and static path sandbox checks.
  - `remote-exec-daemon-xp` is the standalone Windows XP daemon over plain HTTP. It intentionally supports a reduced feature set and no transfer compression.
  - `remote-exec-proto` defines shared public tool schemas and broker-daemon RPC payloads.
  - `remote-exec-admin` provides operator-facing CLI workflows for certificate/bootstrap tasks.
  - `remote-exec-pki` contains reusable certificate generation and manifest helpers.
- The Rust broker/daemon support modern Linux and Windows hosts. Avoid introducing broad new platform abstraction layers unless the current Linux/Windows/XP split actually needs them.

## Workspace Map

- `Cargo.toml`: workspace manifest and shared dependency versions.
- `crates/remote-exec-broker/src/`: public MCP server, target config, daemon client, tool handlers, and session store.
- `crates/remote-exec-daemon/src/`: daemon config, TLS setup, HTTP server, exec session logic, patch engine, image handling, transfer handling, and sandbox enforcement.
- `crates/remote-exec-daemon-xp/`: standalone XP daemon, Win32 exec/session handling, HTTP routes, patch engine, and narrow transfer implementation.
- `crates/remote-exec-proto/src/public.rs`: public tool arguments and structured results.
- `crates/remote-exec-proto/src/rpc.rs`: internal broker-daemon request/response types.
- `crates/remote-exec-proto/src/path.rs`: cross-platform path policy helpers used by transfer and sandbox code.
- `crates/remote-exec-proto/src/sandbox.rs`: shared allow/deny sandbox config and enforcement helpers.
- `crates/remote-exec-admin/src/`: CLI entrypoints for certificate/bootstrap workflows.
- `crates/remote-exec-pki/src/`: shared PKI generation, manifest, and write helpers.
- `configs/*.example.toml`: canonical config examples for broker and daemon shape.
- `README.md`: operator runbook, trust model, bootstrap flow, and project-wide quality gate.
- `tests/e2e/multi_target.rs`: broker-plus-daemon multi-target end-to-end coverage.

## Architecture Rules

- Preserve the split between broker-owned public session IDs and daemon-owned local sessions. Do not expose daemon-local process IDs or session identifiers through the public API.
- Keep target selection explicit for machine-local operations. Broker-side validation and routing are part of the contract.
- Maintain per-target isolation. A session or file operation created on one target must not be usable against another target.
- Preserve the current trust model: choosing a `target` grants broad access on that machine, optionally narrowed only by static allow/deny sandbox config. Do not add interactive approval or sandbox-escalation flows unless the task explicitly expands the security model.
- When a target is temporarily unreachable, prefer the current behavior: broker startup may succeed, and identity/availability is verified before the first forwarded call.
- Preserve the current broker-host `local` semantics:
  - `transfer_files` may use broker-host filesystem access with `target: "local"` even when the broker `[local]` exec target is disabled.
  - broker `[local]` only controls broker-host `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
- Keep `list_targets` broker-local and cache-based. It should report the broker's current cached daemon metadata, including truthful `supports_pty` capability.

## Change Guidance

- When changing public tool arguments, result fields, or validation rules:
  - update `crates/remote-exec-proto/src/public.rs`
  - update the matching broker tool handler(s)
  - update the daemon RPC handler(s) if behavior changed
  - update `README.md`, the relevant config example comments, and `skills/using-remote-exec-mcp/SKILL.md`
  - update relevant tests
- When changing broker-daemon transport or daemon RPC contracts:
  - update `crates/remote-exec-proto/src/rpc.rs`
  - update both the broker client and daemon server/handler paths in the same change
  - update `crates/remote-exec-daemon-xp` too when the plain-HTTP XP path shares that contract
  - keep error messages stable where tests or documented behavior depend on them
- When changing transfer behavior or transfer capability reporting, update the following together if applicable:
  - `crates/remote-exec-broker/src/tools/transfer.rs`
  - `crates/remote-exec-broker/src/tools/targets.rs`
  - `crates/remote-exec-daemon/src/transfer/`
  - `crates/remote-exec-daemon-xp/src/transfer_ops.cpp`
  - `README.md`
  - `crates/remote-exec-daemon-xp/README.md`
  - `skills/using-remote-exec-mcp/SKILL.md`
- When changing sandbox behavior, update the following together if applicable:
  - `crates/remote-exec-proto/src/sandbox.rs`
  - broker `host_sandbox` config / enforcement
  - daemon sandbox config / enforcement
  - `README.md`
  - `configs/*.example.toml`
- When changing certificate/bootstrap behavior, update the following together if applicable:
  - `crates/remote-exec-admin`
  - `crates/remote-exec-pki`
  - `configs/*.example.toml`
  - README TLS/bootstrap instructions
- Prefer focused changes inside the relevant crate instead of broad workspace refactors. The current crate split is deliberate.

## Testing Expectations

- Run targeted tests for the area you changed before running broader workspace checks.
- Relevant focused commands:
  - `cargo test -p remote-exec-daemon --test exec_rpc`
  - `cargo test -p remote-exec-daemon --test patch_rpc`
  - `cargo test -p remote-exec-daemon --test image_rpc`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`
  - `cargo test -p remote-exec-daemon --test health`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_assets`
  - `cargo test -p remote-exec-broker --test mcp_transfer`
  - `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
  - `cargo test -p remote-exec-admin --test dev_init`
  - `cargo test -p remote-exec-admin --test certs_issue`
  - `cargo test -p remote-exec-pki --test dev_init_bundle`
  - `make -C crates/remote-exec-daemon-xp test-host-transfer`
  - `make -C crates/remote-exec-daemon-xp check`
- For cross-cutting or public-surface changes, finish with the full quality gate from `README.md`:
  - `cargo test --workspace`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Editing Notes

- Keep generated artifacts out of the repo. Do not commit files from `target/` or generated certificate/key material.
- `Cargo.lock` is tracked. Update it only when dependency changes require it.
- Prefer updating docs in the same change when behavior, config shape, CLI output, or trust-model wording changes.
- If a change touches user-facing tool behavior, include or extend tests that exercise the public broker surface, not only daemon internals.
- Everything under `docs/` is historical implementation detail and planning context, not the live contract. Do not treat it as the source of truth, and do not rewrite those dated notes unless the task explicitly asks for historical-doc maintenance.
