# Transfer Codec Conformance Design

Status: approved design captured in writing

Date: 2026-05-05

References:

- `crates/remote-exec-proto/src/rpc.rs`
- `crates/remote-exec-broker/src/tools/transfer/codec.rs`
- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-daemon/src/transfer/codec.rs`
- `crates/remote-exec-daemon/src/transfer/mod.rs`
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`

## Goal

Make the internal transfer metadata/header contract explicit, typed, and covered
by cross-implementation conformance tests for the Rust broker, Rust daemon, and
C++ daemon.

The change should remove duplicated Rust enum-header parsing, fix known C++
metadata drift, and keep the public MCP `transfer_files` tool shape stable.

## Scope

This design covers:

- transfer export metadata headers
- transfer import metadata headers
- enum and boolean parsing rules for transfer metadata
- Rust broker transfer codec adapters
- Rust daemon transfer codec adapters
- C++ daemon transfer import/export metadata validation
- conformance coverage across Rust and C++ live implementation paths

This design does not cover:

- changing public MCP `transfer_files` request or result schemas
- redesigning tar archive creation, extraction, or bundling
- changing transfer compression behavior beyond metadata parsing
- replacing HTTP headers with JSON sidecar metadata
- broad broker transfer route refactoring
- broad C++ HTTP parser refactoring
- changing the trust model, sandbox model, target routing, or local target rules

## Problem Statement

Transfer metadata is currently a live broker-daemon RPC contract, but its codec
rules are scattered across the codebase.

The Rust broker parses export response headers and writes import request
headers in `crates/remote-exec-broker/src/tools/transfer/codec.rs`.

The Rust daemon writes export response headers and parses import request
headers in `crates/remote-exec-daemon/src/transfer/codec.rs`.

The C++ daemon parses and writes equivalent headers in
`crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`.

The shared proto crate declares header names and transfer metadata structs, but
it does not define the metadata wire semantics. As a result:

- Rust enum values are parsed with ad hoc JSON-string formatting.
- Broker and daemon Rust codecs duplicate string mappings.
- The C++ daemon can silently accept missing or invalid import metadata and fail
  later in operation code.
- Required/defaulted header behavior is not locked down as a cross-language
  contract.

The current C++ drift is concrete: `x-remote-exec-create-parent` is required by
the Rust daemon import codec, but the C++ daemon currently treats a missing
header as `false`.

## Contract Decisions

### Header Names

Header names remain unchanged:

- `x-remote-exec-source-type`
- `x-remote-exec-compression`
- `x-remote-exec-destination-path`
- `x-remote-exec-overwrite`
- `x-remote-exec-create-parent`
- `x-remote-exec-symlink-mode`

The Rust constants remain in `remote-exec-proto/src/rpc.rs`.

### Transfer Export Metadata

Export metadata is sent by daemon transfer export responses and consumed by the
broker.

Required headers:

- `x-remote-exec-source-type`

Optional headers:

- `x-remote-exec-compression`

Defaults:

- missing `x-remote-exec-compression` means `none`

Accepted `x-remote-exec-source-type` values:

- `file`
- `directory`
- `multiple`

Accepted `x-remote-exec-compression` values:

- `none`
- `zstd`

The broker should still validate that the returned compression matches the
requested compression.

### Transfer Import Metadata

Import metadata is sent by the broker to daemon transfer import routes and
consumed by Rust and C++ daemons.

Required headers:

- `x-remote-exec-destination-path`
- `x-remote-exec-overwrite`
- `x-remote-exec-create-parent`
- `x-remote-exec-source-type`

Optional headers:

- `x-remote-exec-compression`
- `x-remote-exec-symlink-mode`

Defaults:

- missing `x-remote-exec-compression` means `none`
- missing `x-remote-exec-symlink-mode` means `preserve`

Accepted `x-remote-exec-overwrite` values:

- `fail`
- `merge`
- `replace`

Accepted `x-remote-exec-create-parent` values:

- `true`
- `false`

Accepted `x-remote-exec-source-type`, `x-remote-exec-compression`, and
`x-remote-exec-symlink-mode` values match their Rust enum serde names:

- source type: `file`, `directory`, `multiple`
- compression: `none`, `zstd`
- symlink mode: `preserve`, `follow`, `skip`

Any missing required header, invalid boolean, or invalid enum value is a request
fault. Rust daemon responses should continue using the existing `bad_request`
RPC code for malformed headers. C++ daemon responses should align with that
behavior.

## Rust Architecture

### Proto-owned wire semantics

`remote-exec-proto` should own the transfer metadata wire semantics because the
metadata structs and header constants already live there.

Add transport-neutral helpers that do not depend on `reqwest`, `axum`, or
`http`. The helpers should operate on plain header names and string values.

Practical shape:

- `TransferHeaderError`
  - fields: header name, message, and category such as missing or invalid
- enum string helpers
  - `TransferCompression::wire_value()`
  - `TransferCompression::from_wire_value(...)`
  - equivalent helpers for source type, overwrite, and symlink mode
- metadata helpers
  - build import metadata headers from `TransferImportMetadata`
  - build export metadata headers from `TransferExportMetadata`
  - parse import metadata from a header getter
  - parse export metadata from a header getter

The exact API can be adjusted during implementation, but the proto crate must
remain transport-neutral.

### Broker adapter

`crates/remote-exec-broker/src/tools/transfer/codec.rs` should become a thin
adapter between `reqwest` header types and the proto codec helpers.

Responsibilities that remain in the broker adapter:

- converting `reqwest::header::HeaderMap` values to strings
- applying header pairs to `reqwest::RequestBuilder`
- converting proto codec failures to `DaemonClientError::Decode`

Responsibilities that move out:

- enum-to-string mappings
- enum parsing
- import/export metadata default rules

### Rust daemon adapter

`crates/remote-exec-daemon/src/transfer/codec.rs` should become a thin adapter
between Axum/http header types and the proto codec helpers.

Responsibilities that remain in the daemon adapter:

- converting `HeaderMap` values to strings
- applying export header pairs to `Response::builder`
- converting proto codec failures to `(StatusCode, Json<RpcErrorBody>)`

Responsibilities that move out:

- enum-to-string mappings
- enum parsing
- create-parent parsing
- default rules for optional metadata headers

## C++ Architecture

The C++ daemon cannot share Rust helper code, but it should mirror the same
metadata contract explicitly.

`transfer_http_codec.cpp` should own C++ transfer metadata validation.

Required behavior:

- missing required import headers fail before archive import begins
- invalid `create_parent` values fail before archive import begins
- invalid source type, overwrite, compression, and symlink mode values fail
  before archive import begins
- missing compression defaults to `none`
- missing symlink mode defaults to `preserve`
- export responses continue writing `x-remote-exec-source-type` and
  `x-remote-exec-compression`
- unsupported non-`none` compression still reports
  `transfer_compression_unsupported`

The C++ route handler should remain the place that maps `TransferFailure` to
HTTP/RPC responses. The codec should throw explicit `TransferFailure` values
for request faults instead of returning unchecked strings.

## Test Strategy

### Proto unit tests

Add Rust unit tests in `remote-exec-proto` for:

- rendering import metadata header pairs
- rendering export metadata header pairs
- parsing import metadata with all headers present
- parsing import metadata with optional compression and symlink mode omitted
- rejecting each missing required import header
- rejecting invalid `create_parent`
- rejecting invalid enum values
- parsing export metadata with compression omitted
- rejecting missing export source type

These tests are the canonical contract tests for the Rust codec helpers.

### Broker codec tests

Add focused tests for the broker adapter to prove it uses the proto contract:

- parsing export metadata from `reqwest::header::HeaderMap`
- rejecting invalid export source type as a decode error
- applying import metadata headers with the exact canonical values

These tests should stay small because most behavior belongs in proto tests.

### Rust daemon codec or RPC tests

Add focused tests for the Rust daemon path:

- import rejects missing `x-remote-exec-create-parent` as `bad_request`
- import rejects invalid `x-remote-exec-create-parent` as `bad_request`
- import rejects invalid enum metadata as `bad_request`
- import accepts omitted optional compression and symlink mode with defaults

Some existing tests already cover related cases. The implementation should add
only the missing conformance cases.

### C++ daemon conformance tests

Add C++ route tests in `test_server_routes.cpp` for the same import metadata
edge cases:

- missing `x-remote-exec-create-parent` rejects with `bad_request`
- invalid `x-remote-exec-create-parent` rejects with `bad_request`
- invalid source type rejects with `bad_request`
- invalid overwrite rejects with `bad_request`
- invalid symlink mode rejects with `bad_request`
- omitted optional compression and symlink mode still import using defaults

The C++ tests should hit the route layer rather than only the codec helper so
that request parsing, route error mapping, and transfer operation gating are all
covered.

## Compatibility

This tightens the C++ daemon behavior for malformed transfer import requests.
That is intentional because the broker-generated requests already include the
required metadata. Correct broker traffic remains compatible.

Operators or custom clients sending incomplete C++ daemon import requests may
now receive earlier `bad_request` errors. This aligns the C++ daemon with the
Rust daemon and removes a silent fallback.

## Implementation Notes

- Keep public MCP schemas unchanged.
- Keep broker transfer orchestration unchanged except for codec calls.
- Keep Rust daemon transfer operation behavior unchanged after metadata parsing.
- Keep C++ archive import/export behavior unchanged after metadata parsing.
- Avoid adding dependencies to `remote-exec-proto`.
- Keep error messages clear but do not make tests depend on exact full message
  text unless there is an established precedent. Prefer checking RPC code and a
  stable key phrase.

## Verification

Focused verification:

- `cargo test -p remote-exec-proto`
- `cargo test -p remote-exec-broker --test mcp_transfer`
- `cargo test -p remote-exec-daemon --test transfer_rpc`
- `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- `make -C crates/remote-exec-daemon-cpp check-posix`

If implementation touches broader transfer behavior, also run:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Acceptance Criteria

- `remote-exec-proto` owns canonical Rust transfer metadata wire semantics.
- Rust broker and Rust daemon transfer codecs no longer duplicate enum string
  parsing or formatting.
- C++ daemon import metadata validation matches the Rust daemon for required
  headers, optional defaults, invalid booleans, and invalid enum values.
- New conformance tests fail before implementation and pass afterward.
- Public MCP `transfer_files` behavior remains unchanged for valid requests.
