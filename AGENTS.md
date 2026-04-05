# AGENTS.md

## Scope

These instructions apply to the entire `remote-exec-mcp` workspace.

## Project Overview

- This repository is a Rust 2024 workspace for a remote-first MCP server that exposes Codex-style local system tools across multiple Linux machines.
- The public tool surface is currently `exec_command`, `write_stdin`, `apply_patch`, and `view_image`.
- The architecture is intentionally split:
  - `remote-exec-broker` is the public MCP server over stdio. It validates `target`, routes requests to daemons, and owns the opaque public `session_id` namespace.
  - `remote-exec-daemon` is the per-machine mTLS JSON/HTTP server that performs local execution, patching, and image reads.
  - `remote-exec-proto` defines shared public tool schemas and broker-daemon RPC payloads.
  - `remote-exec-admin` provides operator-facing CLI workflows for certificate/bootstrap tasks.
  - `remote-exec-pki` contains reusable certificate generation and manifest helpers.
- Linux-only is an explicit v1 constraint. Do not introduce cross-platform abstractions unless the task explicitly asks for them.

## Workspace Map

- `Cargo.toml`: workspace manifest and shared dependency versions.
- `crates/remote-exec-broker/src/`: public MCP server, target config, daemon client, tool handlers, and session store.
- `crates/remote-exec-daemon/src/`: daemon config, TLS setup, HTTP server, exec session logic, patch engine, and image handling.
- `crates/remote-exec-proto/src/public.rs`: public tool arguments and structured results.
- `crates/remote-exec-proto/src/rpc.rs`: internal broker-daemon request/response types.
- `crates/remote-exec-admin/src/`: CLI entrypoints for certificate/bootstrap workflows.
- `crates/remote-exec-pki/src/`: shared PKI generation, manifest, and write helpers.
- `configs/*.example.toml`: canonical config examples for broker and daemon shape.
- `README.md`: operator runbook, trust model, bootstrap flow, and project-wide quality gate.
- `tests/e2e/multi_target.rs`: broker-plus-daemon multi-target end-to-end coverage.

## Architecture Rules

- Preserve the split between broker-owned public session IDs and daemon-owned local sessions. Do not expose daemon-local process IDs or session identifiers through the public API.
- Keep target selection explicit for machine-local operations. Broker-side validation and routing are part of the contract.
- Maintain per-target isolation. A session or file operation created on one target must not be usable against another target.
- Preserve the v1 trust model: choosing a `target` is equivalent to full access on that machine. Do not add partial approval or sandbox flows unless the task explicitly expands the security model.
- When a target is temporarily unreachable, prefer the current behavior: broker startup may succeed, and identity/availability is verified before the first forwarded call.

## Change Guidance

- When changing public tool arguments, result fields, or validation rules:
  - update `crates/remote-exec-proto/src/public.rs`
  - update the matching broker tool handler(s)
  - update the daemon RPC handler(s) if behavior changed
  - update README and relevant tests
- When changing broker-daemon transport or daemon RPC contracts:
  - update `crates/remote-exec-proto/src/rpc.rs`
  - update both the broker client and daemon server/handler paths in the same change
  - keep error messages stable where tests or documented behavior depend on them
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
  - `cargo test -p remote-exec-daemon --test health`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_assets`
  - `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
  - `cargo test -p remote-exec-admin --test dev_init`
  - `cargo test -p remote-exec-admin --test dev_init_cli`
  - `cargo test -p remote-exec-pki --test dev_init_bundle`
- For cross-cutting or public-surface changes, finish with the full quality gate from `README.md`:
  - `cargo test --workspace`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Editing Notes

- Keep generated artifacts out of the repo. Do not commit files from `target/` or generated certificate/key material.
- `Cargo.lock` is tracked. Update it only when dependency changes require it.
- Prefer updating docs in the same change when behavior, config shape, CLI output, or trust-model wording changes.
- If a change touches user-facing tool behavior, include or extend tests that exercise the public broker surface, not only daemon internals.
