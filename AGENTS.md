# AGENTS.md

## Scope

These instructions apply to the entire `remote-exec-mcp` workspace.

Everything under `docs/` is historical planning and audit context unless a task
explicitly asks for docs maintenance there. The live contract is maintained in:

- `README.md`
- `AGENTS.md`
- `configs/*.example.toml`
- `skills/using-remote-exec-mcp/SKILL.md`
- public schemas in `crates/remote-exec-proto/src/`

## Project Overview

This repository is a Rust 2024 workspace for a remote-first MCP server that
exposes Codex-style local-system tools across configured Linux and Windows
targets. It also contains a narrower standalone C++11 daemon with native POSIX
and Windows XP-compatible build paths. In this repository, "Windows
XP-compatible" means a toolchain and binary target that can both compile the C++
daemon as C++11 and target Windows XP.

The public tool surface is:

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`
- `forward_ports`

The architecture is intentionally split:

- `remote-exec-broker`: public MCP server over stdio or streamable HTTP. It
  validates `target`, routes calls, owns public `session_id` and `forward_id`
  namespaces, and hosts the `remote-exec` CLI client implementation.
- `remote-exec-host`: shared Rust host runtime used by the Rust daemon and
  broker-host `local` behavior. It owns transport-neutral exec, patch, image,
  transfer, path, sandbox, and port-forward host logic.
- `remote-exec-daemon`: per-machine Rust HTTP/1.1 JSON daemon. It supports mTLS
  by default, optional plain HTTP, command sessions, patching, image reads,
  transfer import/export, sandbox checks, and v4 port-forward upgrade tunnels.
- `remote-exec-daemon-cpp`: standalone plain-HTTP C++11 daemon. It shares the
  broker-daemon contract where implemented, supports native POSIX and Windows
  XP-compatible builds, and intentionally omits TLS and transfer compression.
- `remote-exec-proto`: shared public tool schemas, internal RPC payloads, path
  helpers, sandbox helpers, and port-forward protocol types.
- `remote-exec-admin`: operator CLI for certificate/bootstrap workflows.
- `remote-exec-pki`: reusable certificate generation, manifest, and secure
  private-key write helpers.

Avoid broad new abstraction layers unless the existing Linux, Windows, POSIX
C++11, and XP-targeting C++11 split actually needs them.

## Workspace Map

- `Cargo.toml`: Rust workspace manifest and shared dependency versions.
- `crates/remote-exec-broker/src/main.rs`: broker MCP server entrypoint.
- `crates/remote-exec-broker/src/bin/remote_exec.rs`: `remote-exec` CLI.
- `crates/remote-exec-broker/src/mcp_server.rs`: MCP tool registration and
  stdio/HTTP serving.
- `crates/remote-exec-broker/src/client.rs`: CLI/client-side tool invocation.
- `crates/remote-exec-broker/src/tools/`: public tool handlers.
- `crates/remote-exec-broker/src/port_forward/`: broker-owned forward store,
  supervision, TCP/UDP bridges, reconnect state, and limits.
- `crates/remote-exec-broker/tests/`: public broker, CLI, transfer, TLS, HTTP,
  port-forward, and multi-target coverage.
- `crates/remote-exec-host/src/`: shared host runtime.
- `crates/remote-exec-host/src/exec/`: host-local command/session backends.
- `crates/remote-exec-host/src/transfer/`: transfer archive import/export.
- `crates/remote-exec-host/src/port_forward/`: daemon-side v4 tunnel session
  and TCP/UDP endpoint runtime.
- `crates/remote-exec-daemon/src/`: Rust daemon config, TLS, HTTP routes, and
  RPC handler glue around `remote-exec-host`.
- `crates/remote-exec-daemon/tests/`: Rust daemon RPC coverage.
- `crates/remote-exec-daemon-cpp/`: C++ daemon source, public headers, config
  example, GNU/BSD/NMAKE build files, and C++ tests.
- `crates/remote-exec-proto/src/public.rs`: MCP public arguments and results.
- `crates/remote-exec-proto/src/rpc.rs` and `src/rpc/`: broker-daemon RPC
  request/response and typed error contracts.
- `crates/remote-exec-proto/src/port_forward.rs`: v4 frame and endpoint helpers.
- `crates/remote-exec-proto/src/path.rs`: cross-platform path policy helpers.
- `crates/remote-exec-proto/src/sandbox.rs`: shared allow/deny sandbox helpers.
- `crates/remote-exec-admin/src/`: certificate/bootstrap CLI.
- `crates/remote-exec-pki/src/`: PKI generation and manifest helpers.
- `configs/*.example.toml`: canonical Rust broker/daemon config examples.
- `tests/support/`: shared test support outside a single crate.
- `portable-pty/` and `winptyrs/`: vendored/local PTY support crates.

## Architecture Rules

- Preserve broker-owned public ID namespaces. Public `session_id` and
  `forward_id` values are opaque broker runtime tokens. Never expose
  daemon-local process IDs, daemon-local session IDs, stream IDs, or tunnel
  internals through the public API.
- Keep target selection explicit for machine-local operations. Broker-side
  validation and routing are part of the contract.
- Maintain per-target isolation. A session, file operation, or forward created
  for one target must not be usable as another target.
- Keep `list_targets` broker-local and cache-based. It must not probe daemons at
  read time. It should report current cached daemon metadata truthfully,
  including `supports_pty`, `supports_port_forward`, and
  `port_forward_protocol_version` when known.
- Preserve startup tolerance for unreachable targets. Broker startup may
  succeed with temporarily unavailable daemons; identity and availability are
  verified before the first forwarded call.
- Preserve the trust model: choosing a target grants broad access on that
  machine, optionally narrowed only by static allow/deny sandbox config for
  path-based operations. Do not add interactive approval or sandbox-escalation
  flows unless the task explicitly changes the security model.
- Preserve broker-host `local` semantics:
  - `[local]` enables `target: "local"` for `exec_command`, `write_stdin`,
    `apply_patch`, and `view_image`.
  - `transfer_files` may use broker-host filesystem access with
    `target: "local"` even when `[local]` is omitted.
  - `forward_ports` may use broker-host network access with side `"local"` even
    when `[local]` is omitted.
  - Broker `host_sandbox` applies to broker-host filesystem access, not to
    `forward_ports` network access.
- Preserve v4 port forwarding. The live daemon-private tunnel protocol uses
  `X-Remote-Exec-Port-Tunnel-Version: 4`. Legacy versions can remain reserved
  but unsupported.
- Treat `forward_ports` `phase` as the precise live state. Legacy
  `status = "open"` can coexist with `phase = "reconnecting"`.
- Forwarding reconnect preserves the forward and future listen-side traffic when
  only broker-daemon transport is lost and the daemon stays alive. Active TCP
  streams and UDP per-peer connector state are not preserved.
- Keep Rust daemon and C++ daemon behavior aligned where they share a public or
  broker-daemon contract. If C++ intentionally lacks a feature, report that
  truthfully through target metadata or documented errors.

## Change Guidance

When changing public tool arguments, result fields, validation, text output, or
structured-content behavior, update these together:

- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/src/client.rs`
- `crates/remote-exec-broker/src/tools/registry.rs`
- the relevant broker tool handler under `crates/remote-exec-broker/src/tools/`
- `crates/remote-exec-broker/src/bin/remote_exec.rs` when the CLI surface should
  expose the behavior
- Rust daemon RPC routes/handlers when daemon behavior changes
- `crates/remote-exec-daemon-cpp` when the C++ daemon shares the contract
- `README.md`, `configs/*.example.toml` when config or behavior changes, and
  `skills/using-remote-exec-mcp/SKILL.md` for user-facing tool changes
- public broker tests, not only daemon internals

When changing broker-daemon RPC contracts:

- update `crates/remote-exec-proto/src/rpc.rs` or `src/rpc/`
- update `crates/remote-exec-broker/src/daemon_client.rs`
- update Rust daemon HTTP route/handler paths
- update `remote-exec-host` if the host runtime owns the behavior
- update `remote-exec-daemon-cpp` if the C++ daemon shares that route
- keep typed RPC error codes stable where retry, normalization, or tests depend
  on them

When changing `forward_ports`:

- update `crates/remote-exec-proto/src/public.rs`
- update `crates/remote-exec-proto/src/port_forward.rs`
- update `crates/remote-exec-broker/src/tools/port_forward.rs`
- update `crates/remote-exec-broker/src/port_forward/`
- update `crates/remote-exec-host/src/port_forward/`
- update `crates/remote-exec-daemon/src/port_forward.rs`
- update `crates/remote-exec-daemon-cpp/include/port_tunnel*.h` and
  `crates/remote-exec-daemon-cpp/src/port_tunnel*.cpp` when C++ shares the
  behavior
- update broker, daemon, C++ tests, README, config examples, and the skill

When changing transfer behavior or capability reporting:

- update `crates/remote-exec-broker/src/tools/transfer.rs`
- update `crates/remote-exec-broker/src/tools/targets.rs` if metadata changes
- update `crates/remote-exec-host/src/transfer/`
- update `crates/remote-exec-daemon/src/transfer/` route glue if needed
- update `crates/remote-exec-daemon-cpp/src/transfer_ops*.cpp`
- update README, C++ README, config comments, skill, and transfer tests

When changing sandbox behavior:

- update `crates/remote-exec-proto/src/sandbox.rs`
- update broker `host_sandbox` config/enforcement
- update Rust daemon sandbox config/enforcement
- update C++ daemon sandbox config/enforcement if applicable
- update README and config examples

When changing certificate/bootstrap behavior:

- update `crates/remote-exec-admin`
- update `crates/remote-exec-pki`
- update README TLS/bootstrap instructions and config examples

Prefer focused changes inside the responsible crate. Do not use broad refactors
to hide a contract change.

## Testing Expectations

Run targeted tests for the area changed before broader checks.

Focused Rust commands:

- `cargo test -p remote-exec-daemon --test exec_rpc`
- `cargo test -p remote-exec-daemon --test patch_rpc`
- `cargo test -p remote-exec-daemon --test image_rpc`
- `cargo test -p remote-exec-daemon --test transfer_rpc`
- `cargo test -p remote-exec-daemon --test port_forward_rpc`
- `cargo test -p remote-exec-daemon --test health`
- `cargo test -p remote-exec-broker --test mcp_exec`
- `cargo test -p remote-exec-broker --test mcp_assets`
- `cargo test -p remote-exec-broker --test mcp_transfer`
- `cargo test -p remote-exec-broker --test mcp_forward_ports`
- `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- `cargo test -p remote-exec-broker --test mcp_cli`
- `cargo test -p remote-exec-broker --test mcp_http`
- `cargo test -p remote-exec-broker --test mcp_tls`
- `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- `cargo test -p remote-exec-admin --test dev_init`
- `cargo test -p remote-exec-admin --test certs_issue`
- `cargo test -p remote-exec-pki --test dev_init_bundle`

Focused C++ commands:

- `make -C crates/remote-exec-daemon-cpp check-posix`
- `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- `bmake -C crates/remote-exec-daemon-cpp check-posix` when validating the BSD
  make path
- `nmake /f crates\remote-exec-daemon-cpp\NMakefile check-msvc-xp` from an x86
  Visual Studio developer prompt with an XP-capable C++11 toolset

Useful Windows cross-target compile gates from Linux:

- `cargo check --workspace --all-targets --all-features --target x86_64-pc-windows-gnu`
- `cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings`
- `cargo build --workspace --all-targets --all-features --target x86_64-pc-windows-gnu`

Do test under wine if that is needed and wine is available.

For cross-cutting or public-surface changes, finish with the quality gate from
`README.md`:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- relevant C++ checks for touched C++ code

## Editing Notes

- Keep generated artifacts out of the repo. Do not commit `target/`,
  `crates/remote-exec-daemon-cpp/build/`, generated certificates, private keys,
  or ad hoc test output.
- `Cargo.lock` is tracked. Update it only when dependency changes require it.
- Do not rewrite historical files under `docs/` unless the task explicitly asks
  for historical-doc maintenance.
- If user-facing behavior changes, include or update public broker-surface tests
  and docs in the same change.
- For C++ daemon work, keep GNU make, BSD make, and NMAKE entry points aligned
  only where they intentionally support the same build path. BSD make is
  POSIX-only.
- For Windows behavior, verify Windows-only Rust code with an appropriate local
  Windows target when CI failure risk is plausible.
