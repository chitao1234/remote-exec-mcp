# Cross-Language Codebase Quality Refactor Design

**Date:** 2026-05-04

## Goal

Improve code quality and structure across the `remote-exec-mcp` workspace with a semi-aggressive internal refactor that reduces future change cost across the broker, Rust daemon, C++ daemon, and test harnesses, while keeping the public MCP tool schemas stable.

## Constraints

- Keep public MCP tool names, request schemas, and response schemas stable.
- Keep documented public behavior stable unless there is a strong internal-consistency reason to tighten behavior that already matches tests or docs.
- Keep `exec_command` patch interception behavior intact.
- Preserve the broker-owned public session ID contract and the daemon-owned local session contract.
- Preserve per-target isolation and explicit target routing.
- Preserve current trust-model assumptions and sandbox model.
- Preserve the current Linux/Windows/XP split instead of introducing a large cross-platform abstraction framework.

## Non-Goals

- No redesign of the public MCP tool surface.
- No removal of `exec_command` patch interception compatibility.
- No attempt to unify Rust and C++ implementations into shared source or generated bindings.
- No large `remote-exec-host` rewrite unless a specific host-runtime seam blocks the broker/daemon cleanup.
- No rewrite of historical documents under `docs/` that are not part of the live contract.

## Problem Statement

The workspace is healthy from a correctness and hygiene perspective, but the current structure has accumulated a high refactor tax:

- The broker still contains repeated local-vs-remote dispatch logic across target operations and port forwarding.
- Transfer metadata parsing and formatting are duplicated across the broker, Rust daemon, and C++ daemon transport layers.
- The Rust daemon is partly transport-thin, but several modules still mix request parsing, validation, logging, response formatting, and host-runtime invocation in the same file.
- The C++ daemon has grown large route and port-forward monoliths that are increasingly expensive to reason about and modify safely.
- The test harnesses provide strong coverage, but repeated helper implementations for archive building, transfer capture, and stub-daemon behavior increase future maintenance cost.

The codebase therefore does not primarily need defect repair. It needs stronger module boundaries so that future feature work and bug fixes can be made without editing several parallel call paths and large monolithic files.

## Current Hotspots

### Broker

- `crates/remote-exec-broker/src/lib.rs`
  - Mixes target backend representation, identity verification, cache handling, broker state, startup assembly, and logging.
- `crates/remote-exec-broker/src/local_backend.rs`
  - Repeats a broad set of capability entrypoints that mirror remote daemon client entrypoints.
- `crates/remote-exec-broker/src/port_forward.rs`
  - Recreates the same local-vs-remote dispatch pattern instead of sharing a common broker-side backend capability model.
- `crates/remote-exec-broker/src/daemon_client.rs`
  - Combines daemon HTTP transport logic with transfer-specific header encoding and decoding concerns.

### Rust daemon

- `crates/remote-exec-daemon/src/transfer/mod.rs`
  - Still combines transport parsing, transport formatting, and host-runtime invocation.
- The Rust daemon generally has the right direction, but feature modules are not yet consistently shaped as transport shells around host-runtime operations.

### C++ daemon

- `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
  - Centralizes request dispatch plus substantial per-feature logic for exec, patch, image, transfer, and port-forward operations.
- `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
  - Combines endpoint parsing, socket handling, ID allocation, payload encoding, and connection lifecycle code in one large module.

### Tests

- `crates/remote-exec-broker/tests/mcp_transfer.rs`
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- `crates/remote-exec-broker/tests/support/stub_daemon.rs`

These files contain duplicated archive and transfer helper logic that should live in reusable support modules.

## Refactor Strategy

Use a capability-core refactor strategy:

1. Clean up broker orchestration boundaries first.
2. Extract reusable test support before the most cross-cutting transport refactors.
3. Localize transfer transport codec responsibilities in both Rust and C++.
4. Thin both daemons into per-feature transport shells around operational cores.
5. Isolate port-forwarding into explicit modules that match the same architectural style as the other capabilities.

This is semi-aggressive because it permits internal type moves, file moves, module splits, and RPC-helper refactors across crates and languages, but it does not alter the public MCP schema contract.

## Target Architecture

### Broker architecture

The broker should remain the sole orchestration layer for target selection, session mapping, and public error shaping, but it should stop acting as a broad hand-written dispatch table.

Targeted shape:

- `state`
  - Broker runtime state, session store, port-forward store, and high-level accessors.
- `startup`
  - Config normalization, sandbox compilation, local target insertion, remote target insertion, and startup logging.
- `target`
  - Target backend representation, target handle behavior, identity verification, cached daemon info, and backend capability dispatch.
- `tools`
  - MCP tool handlers that depend on narrower broker capability interfaces rather than directly branching on local vs remote.

The broker tools should be able to depend on internal capability traits or narrow adapter structs such as:

- `ExecBackend`
- `PatchBackend`
- `ImageBackend`
- `TransferBackend`
- `PortForwardBackend`

The exact Rust mechanism can be traits, enums with focused methods, or small adapter structs. The important requirement is that the repeated `match Remote/Local` pattern is centralized instead of copied across unrelated tool flows.

### Rust daemon architecture

Each Rust daemon feature module should converge on the same shape:

1. Request extraction and transport validation
2. Host-runtime invocation
3. Response and RPC error mapping

The host runtime remains the operational core. The daemon remains transport-specific. The refactor should reduce mixing of:

- HTTP headers and request-body parsing
- tracing/logging decisions
- host-runtime calls
- response header construction

### C++ daemon architecture

The C++ daemon should mirror the Rust daemon shape conceptually even though the implementation remains native C++:

1. Route dispatch
2. Feature-specific request parsing and validation
3. Feature-specific operation code
4. Response formatting and RPC error shaping

The primary split targets are:

- route dispatcher vs feature handlers
- transfer helpers vs transfer routes
- image helpers vs image route
- exec helpers vs exec routes
- port-forward endpoint parsing vs socket handling vs connection state

This preserves XP-compatible implementation constraints while making the code easier to test and review.

## Workstreams

### Workstream 1: Broker core decomposition

Refactor the broker into smaller modules so that state assembly, target backend behavior, identity verification, cache invalidation, and tool orchestration are clearly separated.

Expected outcomes:

- Smaller `lib.rs`
- Shared internal backend capability handling
- Reduced duplication between ordinary target tools and port-forwarding code

### Workstream 2: Test harness extraction

Before touching the most cross-cutting transport paths, extract shared helpers for:

- archive/tar construction
- transfer capture and decoding
- stub-daemon responses
- shared broker/daemon transfer fixtures

Expected outcomes:

- Smaller transfer test files
- Less repeated helper logic
- Lower change cost for later transfer refactors

### Workstream 3: Transfer boundary cleanup

Keep the MCP `transfer_files` schema stable, but localize internal transport codec logic.

Rust side:

- Split transport header parsing and formatting from operational transfer coordination
- Keep broker tool orchestration separate from daemon HTTP codec details

C++ side:

- Split transfer request parsing, transfer response shaping, sandbox/path authorization, and archive import/export handling into focused modules

Expected outcomes:

- One obvious place per language for transfer transport encoding and decoding
- Less duplication between broker, Rust daemon, and C++ daemon

### Workstream 4: Daemon transport-shell thinning

Normalize both daemon implementations so the operational core is not hidden inside large route handlers.

Expected outcomes:

- Smaller Rust feature handler modules
- Smaller C++ route files
- Easier parity work between Rust and C++ implementations

### Workstream 5: Port-forward isolation

Port forwarding should stop being a parallel architecture living beside the rest of the system.

Broker side:

- Use the same backend-capability style used for the other broker-managed operations

C++ side:

- Split endpoint parsing
- Split socket creation/bind/connect
- Split state tracking and ID generation
- Split payload encoding and decoding helpers

Expected outcomes:

- Smaller port-forward modules
- Less duplicated local-vs-remote orchestration logic in the broker
- Safer future work in the C++ daemon

## Sequencing

### Phase 1: Broker structure first

- Split broker state/startup/target responsibilities
- Centralize local-vs-remote dispatch patterns
- Avoid transfer contract changes in this phase

Reason:

This creates a cleaner control surface before the cross-cutting transfer and daemon refactors.

### Phase 2: Test harness extraction

- Centralize repeated test helpers
- Reduce duplication in transfer-heavy tests

Reason:

Later refactors need cleaner verification scaffolding.

### Phase 3: Transfer boundary cleanup

- Refactor Rust broker transfer codec boundaries
- Refactor Rust daemon transfer transport shell
- Refactor C++ transfer route structure

Reason:

Transfer is the largest cross-language contract hot spot after broker dispatch itself.

### Phase 4: Daemon shell thinning

- Normalize Rust daemon feature handlers
- Split C++ route handlers by feature

Reason:

This phase becomes simpler once transfer-specific cleanup has already reduced route complexity.

### Phase 5: Port-forward isolation

- Refactor broker orchestration and C++ port-forward structure

Reason:

Port forwarding deserves first-class module boundaries, but it should follow the shared broker/daemon cleanup patterns established earlier.

## Allowed Internal Breaking Changes

- Move and rename internal modules freely.
- Move and rename internal Rust types freely.
- Change internal helper APIs and internal RPC-transport helper structure freely.
- Change C++ file layout and internal function boundaries freely.
- Split test files and support modules freely.

## Stability Requirements

- Keep MCP tool schemas stable.
- Keep `exec_command` patch interception behavior intact.
- Keep the broker-owned opaque public `session_id` contract intact.
- Keep daemon-local session identifiers internal.
- Keep per-target session and file-operation isolation intact.
- Keep `list_targets` broker-local and cache-based.
- Keep documented trust-model assumptions intact.

## Error Handling Expectations

The refactor should improve consistency of internal error shaping without changing public schema:

- Broker-side target verification and cache invalidation should happen through shared code paths.
- Rust daemon transport modules should map host-runtime errors through a consistent adapter layer.
- C++ feature handlers should produce errors through focused helpers instead of repeating inline status/code/message handling.
- Transfer transport parsing failures should remain explicit and localized to codec layers.

## Testing Strategy

Each phase should end with the narrowest relevant focused tests first, then broader gates.

Focused verification examples:

- Broker:
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_assets`
  - `cargo test -p remote-exec-broker --test mcp_transfer`
  - `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Rust daemon:
  - `cargo test -p remote-exec-daemon --test exec_rpc`
  - `cargo test -p remote-exec-daemon --test patch_rpc`
  - `cargo test -p remote-exec-daemon --test image_rpc`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`
  - `cargo test -p remote-exec-daemon --test health`
  - `cargo test -p remote-exec-daemon --test port_forward_rpc`
- C++ daemon:
  - `make -C crates/remote-exec-daemon-cpp test-host-transfer`
  - `make -C crates/remote-exec-daemon-cpp check-posix`

Cross-cutting finish gate:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Risks

### Scope spread

This plan touches broker, Rust daemon, C++ daemon, and tests. Without careful sequencing it can turn into several partially-finished refactors at once.

Mitigation:

- Use phase boundaries that are independently verifiable.
- Avoid starting multiple deep refactors before the prior phase is green.

### Hidden behavioral coupling

Some behaviors that look duplicated may encode subtle compatibility expectations, especially in transfer and port-forward flows.

Mitigation:

- Extract tests before changing boundaries.
- Keep scenario coverage intact while moving transport and dispatch logic.

### C++ parity drift

The C++ daemon can easily drift from the Rust daemon if the refactor only improves one side.

Mitigation:

- Apply the same architectural shape to both sides even when implementations differ.
- Treat Rust and C++ transport cleanup as one workstream with mirrored outcomes.

## Success Criteria

- Public MCP schemas remain unchanged.
- `exec_command` patch interception remains intact.
- Broker dispatch duplication is materially reduced.
- Transfer transport encoding and decoding are localized instead of repeated across several unrelated modules.
- Rust daemon feature handlers are consistently transport-thin.
- C++ route and port-forward monoliths are split into feature-focused modules.
- Test helper duplication is reduced enough that future transport refactors mainly touch shared support layers instead of several giant test files.
