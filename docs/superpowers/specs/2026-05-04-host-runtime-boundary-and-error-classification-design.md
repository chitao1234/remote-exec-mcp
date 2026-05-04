# Host Runtime Boundary And Error Classification Design

Status: approved design captured in writing

Date: 2026-05-04

References:

- `crates/remote-exec-broker/Cargo.toml`
- `crates/remote-exec-broker/src/lib.rs`
- `crates/remote-exec-broker/src/config.rs`
- `crates/remote-exec-broker/src/local_backend.rs`
- `crates/remote-exec-broker/src/local_transfer.rs`
- `crates/remote-exec-broker/src/port_forward.rs`
- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/src/tools/transfer.rs`
- `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- `crates/remote-exec-daemon/src/lib.rs`
- `crates/remote-exec-daemon/src/server.rs`
- `crates/remote-exec-daemon/src/transfer/mod.rs`
- `crates/remote-exec-daemon-cpp/src/server.cpp`
- `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`

## Goal

Fix the internal broker/daemon boundary so the broker no longer depends directly
on daemon internals for broker-host `local` behavior, and replace
message-sniffing error classification with explicit internal error categories for
both daemon implementations.

The target is architectural cleanup and protocol safety while preserving the
current public tool behavior and trust model.

## Scope

This design covers only the following changes:

- introduce a new internal Rust crate that owns reusable broker-host runtime
  behavior
- make both `remote-exec-daemon` and `remote-exec-broker` depend on that new
  crate instead of the broker importing daemon internals directly
- move Rust host-local exec, patch, image, transfer, and port-forward runtime
  logic behind the new crate boundary
- replace Rust substring-based transfer and image error classification with
  typed domain errors
- replace C++ substring-based transfer and image error classification with
  explicit internal error categories that map to the same public RPC codes
- update broker-side handling to rely on stable RPC codes instead of inferring
  semantics from free-form message text where practical
- preserve the current public behavior for `target: "local"`

This design does not cover:

- public tool argument shape changes
- removal of `transfer_files.source` or `write_stdin.target`
- changes to broker target-selection semantics
- changes to session ID ownership or routing semantics
- transport redesign for daemon HTTP or broker MCP
- TLS/bootstrap redesign
- broad cleanup of every large file in the workspace

## Current Problem Summary

The current crate boundary is inverted for broker-host `local` support.

Today:

- `remote-exec-broker` depends directly on `remote-exec-daemon`
- broker config imports daemon config/runtime types
- broker-local execution, transfer, and port-forward behavior call daemon
  modules directly in process
- broker feature flags are coupled to daemon features such as TLS and winpty

This causes several structural problems:

- the broker cannot evolve independently from daemon runtime internals
- host-local runtime logic is owned by the daemon crate even though the broker
  also uses it directly
- capability additions require duplicated adapter code across broker local,
  broker remote, and daemon-facing layers
- tests reach through the same porous boundary, which increases maintenance cost

Error classification has a similar problem:

- Rust transfer RPC classification in `remote-exec-daemon` derives public error
  codes from substring matching on formatted messages
- broker transfer path-info handling uses message inspection to infer missing or
  unsupported cases
- C++ transfer and image handlers also derive public codes from message text

This makes public behavior dependent on incidental wording. A message rewrite
can accidentally change RPC semantics.

## Decision Summary

### 1. Introduce a new internal crate: `remote-exec-host`

Create a new Rust workspace crate named `remote-exec-host`, as the
single owner of reusable broker-host runtime behavior.

This crate will hold the host-local logic currently trapped inside
`remote-exec-daemon` but consumed by both the daemon and the broker:

- runtime/app state for host-local operations
- host runtime configuration types
- default shell resolution and shell-policy helpers
- exec session spawning, polling, and session-store logic
- patch engine and patch application entry points
- image read logic
- transfer archive import/export/path-info logic
- port-forward state and host-local port RPC operations
- target-info capability shaping helpers used by both broker-local and daemon

The daemon crate remains responsible for:

- HTTP routing
- HTTP auth middleware
- TLS server startup
- transport-specific request/response mapping

The broker remains responsible for:

- MCP tool handling
- target validation and target routing
- public session ID ownership
- broker-local config parsing
- broker-host transfer bundling/orchestration semantics

### 2. Reverse the dependency direction

After the refactor:

- `remote-exec-broker` depends on `remote-exec-host` and `remote-exec-proto`
- `remote-exec-daemon` depends on `remote-exec-host` and `remote-exec-proto`
- `remote-exec-broker` does not depend on `remote-exec-daemon`

This is the main architectural correction. The daemon becomes a transport
wrapper around the host runtime instead of the broker embedding daemon internals.

### 3. Keep broker-host `local` behavior in-process

Broker-host `local` support should keep its current in-process behavior.

This refactor is not an RPC-purity project. The broker will continue to execute
its `local` target path in-process, but it will do so through the extracted host
runtime crate rather than by reaching into daemon modules.

That preserves current behavior while fixing ownership and layering.

### 4. Use explicit typed domain errors in Rust

Rust host-runtime capabilities should return typed errors that encode machine
meaning before formatting human text.

At minimum:

- `TransferError`
- `ImageError`

Each typed error should be mapped exactly once to the public RPC envelope in the
daemon transport layer.

Required stable transfer categories:

- `SandboxDenied`
- `PathNotAbsolute`
- `DestinationExists`
- `ParentMissing`
- `DestinationUnsupported`
- `CompressionUnsupported`
- `SourceUnsupported`
- `SourceMissing`
- `Internal`

Required stable image categories:

- `SandboxDenied`
- `InvalidDetail`
- `Missing`
- `NotFile`
- `DecodeFailed`
- `Internal`

Messages remain human-readable and path-rich, but RPC code selection comes from
the error variant rather than message parsing.

### 5. Mirror explicit error categories in the C++ daemon

The C++ daemon should introduce explicit internal error categories for transfer
and image flows as well.

This can be lighter-weight than the Rust version, for example:

- `enum class TransferErrorCode`
- `enum class ImageErrorCode`

Handlers should construct an explicit code first, then format the response
message second. The public broker-visible codes should remain aligned with the
Rust daemon.

### 6. Preserve the public contract

This refactor is allowed to break internal APIs and crate topology, but it
should preserve the public behavior contract:

- no public tool-shape changes
- no target-selection changes
- no session ownership changes
- no trust-model changes
- no interactive-approval or escalation model changes
- no broker-side exposure of daemon-local IDs

## Target Architecture

### New crate layout

`remote-exec-host` should own the reusable host runtime. A practical initial
module split is:

- `config`
  - host-runtime-only config types and validation helpers
- `state`
  - shared runtime state object and capability helpers
- `host_path`
  - host path-policy and normalization helpers
- `exec`
  - shell resolution, session store, spawn/poll/write operations
- `patch`
  - patch parser/engine/application entry points
- `image`
  - image read and validation logic
- `transfer`
  - path info, archive import/export, summary handling
- `port_forward`
  - listener/connection state and local RPC operations
- `target_info`
  - target-info response shaping
- `error`
  - shared typed error definitions and transport mapping helpers where useful

`remote-exec-daemon` then becomes:

- daemon transport config
- Axum routes
- auth middleware
- TLS/plain HTTP server startup
- mapping from HTTP request/response to `remote-exec-host` calls

`remote-exec-broker` then becomes:

- MCP transport and tool handlers
- remote daemon HTTP client
- local target adapter built on `remote-exec-host`
- broker-local transfer orchestration
- public session store and port-forward registry

### Config boundary

Some current daemon config types mix transport concerns and host runtime
concerns. That should be split instead of copied unchanged.

Host runtime config belongs in `remote-exec-host`:

- default workdir
- windows POSIX root
- shell policy
- PTY mode
- default shell
- host process environment
- yield-time policy
- host sandbox
- host transfer compression support
- patch encoding autodetect flag

Daemon-only transport config stays in `remote-exec-daemon`:

- listen address
- daemon transport mode
- TLS material
- bearer auth

Broker-only config stays in `remote-exec-broker`:

- target URLs
- broker MCP transport
- broker-local enablement
- broker host sandbox

### Local target adapter

Broker local-target support should become a thin adapter over
`remote-exec-host`, not a reimplementation or a daemon import.

That adapter should own:

- building a host runtime from broker local config
- translating host-runtime errors into broker-side `DaemonClientError`-style
  results where needed
- exposing the same capability surface as the broker uses for remote targets

The duplicated local port-forward setup in the broker should be folded into the
same host-runtime-based local adapter rather than maintaining a separate
hand-built in-process daemon state.

## Error Model

### Rust transfer errors

Rust transfer operations should stop returning generic `anyhow::Error` values to
the daemon transport layer for expected failure modes.

Instead:

- archive/path-info/import/export logic returns `Result<T, TransferError>`
- sandbox failures are converted into `TransferError::SandboxDenied`
- absolute-path validation returns `TransferError::PathNotAbsolute`
- incompatible destination state returns explicit destination variants
- unsupported archive/source content returns `SourceUnsupported`
- missing source paths return `SourceMissing`

The daemon transport layer then maps:

- error variant -> HTTP status
- error variant -> RPC code
- error value -> final message text

This mapping should be explicit and table-like.

### Rust image errors

Image handling should follow the same pattern:

- host runtime returns `Result<T, ImageError>`
- invalid `detail` becomes `InvalidDetail`
- missing path becomes `Missing`
- non-file path becomes `NotFile`
- decode or unsupported-format failures become `DecodeFailed`
- sandbox denial becomes `SandboxDenied`

### Broker-side use of RPC codes

Where broker behavior currently infers semantics from free-form text, it should
prefer stable RPC codes.

For example, transfer path-info probing should decide based on explicit RPC code
categories instead of checking whether an error string contains `404`, `405`, or
`unknown endpoint`.

Broker-side fallback message handling can remain for compatibility where needed,
but stable codes should become the primary path.

### C++ error categories

C++ daemon transfer and image handlers should move from:

- catching a generic exception
- deriving the public code from `what()` text

to:

- constructing an explicit internal error category
- carrying the user-facing message alongside that category
- mapping category directly to the existing public RPC code

This can be implemented with small structs or tagged results. It does not need
Rust-style algebraic data types, but it must stop treating message text as the
source of truth for machine semantics.

## Migration Plan

### Slice 1: extract the reusable Rust host runtime

Create `remote-exec-host` and move existing reusable logic there with minimal
behavior changes.

This slice should:

- move or split runtime config types
- move shared app state and target-info shaping
- move host-local exec, patch, image, transfer, and port-forward logic
- keep daemon route signatures stable by re-export or thin wrapper as needed

This slice should not change broker behavior yet.

### Slice 2: switch broker local behavior to `remote-exec-host`

Update broker local-target support to call `remote-exec-host` directly.

This slice should:

- replace direct imports of `remote_exec_daemon::*` runtime modules
- remove separate local port-forward state construction in the broker
- update broker local transfer helpers to use `remote-exec-host`
- remove `remote-exec-daemon` from broker dependencies
- remove daemon feature coupling from broker features where no longer needed

This slice should preserve current `local` behavior and tests.

### Slice 3: typed Rust error model

Introduce typed Rust host-runtime errors and change daemon transport mapping to
use them directly.

This slice should:

- define typed transfer and image errors
- push expected failures into those types
- remove substring-based code classification in Rust daemon transport handlers
- update broker-side code to prefer RPC codes over string inspection

### Slice 4: align the C++ daemon

Update the C++ daemon to use explicit transfer/image error categories that map
to the same public codes.

This slice should:

- replace transfer/image code selection by message sniffing
- keep current public codes stable
- update focused C++ tests to assert stable code behavior

## Code Boundaries

### `remote-exec-broker`

Expected modifications:

- remove direct daemon-runtime imports from `src/config.rs`
- replace `src/local_backend.rs` implementation to use `remote-exec-host`
- replace broker-local transfer helpers in `src/local_transfer.rs`
- fold `src/port_forward.rs` local runtime setup into the shared host runtime
- simplify feature flags and dependencies in `Cargo.toml`

### `remote-exec-daemon`

Expected modifications:

- reduce `src/lib.rs` to host-runtime assembly and daemon wrapper plumbing
- keep `src/server.rs` as daemon transport/router code only
- replace direct transfer/image substring error classification with typed mapping
- use `remote-exec-host` for all host-runtime behavior

### `remote-exec-daemon-cpp`

Expected modifications:

- replace `image_error_code(message)` style routing with explicit image error
  categories
- replace `transfer_error_code(message)` style routing with explicit transfer
  error categories
- preserve existing public RPC code strings and major message shape

### `remote-exec-proto`

Public schemas stay stable unless the implementation discovers a strict need for
an internal-only support code. This design assumes no public schema change.

If the broker benefits from more stable remote classification, it should prefer
existing RPC `code` fields rather than introducing new public fields.

## Testing Strategy

Verification should stay behavior-first.

### Rust host runtime

As logic moves into `remote-exec-host`, focused tests should move with it where
possible so the host runtime is validated independently of daemon HTTP
transport.

The extracted crate should directly own focused tests for:

- exec runtime behavior
- patch application behavior
- image read behavior
- transfer archive behavior
- port-forward local behavior where feasible

### Rust daemon transport

Daemon RPC tests should continue proving that HTTP behavior still matches the
documented contract:

- `cargo test -p remote-exec-daemon --test exec_rpc`
- `cargo test -p remote-exec-daemon --test patch_rpc`
- `cargo test -p remote-exec-daemon --test image_rpc`
- `cargo test -p remote-exec-daemon --test transfer_rpc`
- `cargo test -p remote-exec-daemon --test port_forward_rpc`
- `cargo test -p remote-exec-daemon --test health`

### Broker public surface

Broker tests should keep exercising the MCP/public surface, especially broker
`local` paths:

- `cargo test -p remote-exec-broker --test mcp_exec`
- `cargo test -p remote-exec-broker --test mcp_assets`
- `cargo test -p remote-exec-broker --test mcp_transfer`
- `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

### C++ daemon

Focused C++ tests should cover stable error-code mapping for transfer and image
paths plus existing transport behavior:

- `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- `make -C crates/remote-exec-daemon-cpp check-posix`

### Final quality gate

For the completed refactor, finish with:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Risks And Mitigations

### Risk: accidental public behavior drift for `target: "local"`

Mitigation:

- preserve public broker tests as the source of truth
- keep local-target behavior routed through the same broker tool handlers
- land extraction before behavior changes

### Risk: config splitting creates path-normalization regressions

Mitigation:

- preserve existing config tests
- move runtime-only path normalization with focused tests before simplifying
  wrappers

### Risk: broker still needs fallback handling for mixed daemon versions

Mitigation:

- prefer explicit RPC code handling first
- keep narrow compatibility fallbacks only where older daemon behavior must be
  tolerated
- remove broad text guessing only when the supported surface is covered by tests

### Risk: C++ daemon lags behind Rust error-model cleanup

Mitigation:

- keep C++ alignment as an explicit planned slice, not follow-up debt
- update C++ focused tests in the same change as the classifier rewrite

## Rejected Alternatives

### Keep the current broker -> daemon dependency and only shuffle modules

This is rejected because it does not fix the real boundary problem. It would
reduce file size while preserving the same ownership confusion and feature
coupling.

### Route broker-local behavior through an internal RPC adapter

This is rejected for now because it adds indirection without solving as much of
the ownership problem as a shared host-runtime crate. The broker can keep its
in-process `local` behavior and still have a clean boundary.

### Clean up public tool shapes in the same refactor

This is rejected for this pass because the user asked for internal
architectural cleanup. Public API cleanup can happen later once the internal
boundary is stable.
