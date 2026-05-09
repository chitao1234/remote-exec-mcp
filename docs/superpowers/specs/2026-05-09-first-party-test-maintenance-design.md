# First-Party Test Maintenance Design

Status: approved design captured in writing

Date: 2026-05-09

References:

- `crates/remote-exec-broker/tests/`
- `crates/remote-exec-daemon/tests/`
- `crates/remote-exec-admin/tests/`
- `crates/remote-exec-pki/tests/`
- `crates/remote-exec-daemon-cpp/tests/`
- `tests/support/transfer_archive.rs`
- `README.md`
- `crates/remote-exec-daemon-cpp/Makefile`

## Goal

Expand the previous port-forward and cross-target test maintenance work across
the first-party codebase while preserving important behavioral coverage.

The resulting suite should:

- be easier to navigate and maintain
- contain less duplicated harness and archive-helper code
- keep public broker behavior covered through MCP-facing tests
- keep daemon-local behavior covered through RPC-level tests
- give the C++ daemon test harness real maintenance attention
- avoid production behavior changes unless a test-only cleanup exposes an
  unavoidable support-code bug

## Scope

This pass covers first-party tests only.

In scope:

- Rust workspace crates under `crates/remote-exec-*`
- C++ daemon tests under `crates/remote-exec-daemon-cpp`
- first-party shared test support under `tests/support`
- focused test documentation and planning artifacts for this work

Out of scope:

- `third_party/rust-patches`
- `winptyrs`
- `portable-pty`
- broad production refactors
- changing public tool behavior
- converting the C++ harness to a new test framework

## Maintenance Principles

This pass optimizes for lower long-term maintenance cost, not maximum test-count
reduction.

A test should be removed only when a stronger or equally public nearby test
protects the same behavior. If a test covers a distinct edge case, keep it and
prefer helper extraction, clearer names, or better assertions.

High-value maintenance includes:

- merging tests that share identical setup and assert adjacent fields on the same
  response
- moving duplicated low-level helpers into a shared first-party test helper when
  there are at least two consumers
- splitting or reorganizing very large harness files when that creates clearer
  ownership
- replacing weak assertions with behavior-level assertions
- documenting manual diagnostic tests so they are not mistaken for ordinary
  automated coverage

Low-value maintenance to avoid:

- deleting tests only because a file is large
- centralizing helpers into a giant generic test utility module
- hiding public request shapes in helpers when the request shape is the subject
  of the test
- introducing a C++ test framework only for aesthetic consistency
- touching vendored or excluded dependency trees

## Layered Design

### Broker Public MCP Tests

Target files:

- `crates/remote-exec-broker/tests/mcp_assets.rs`
- `crates/remote-exec-broker/tests/mcp_cli.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `crates/remote-exec-broker/tests/mcp_exec/*.rs`
- `crates/remote-exec-broker/tests/mcp_http.rs`
- `crates/remote-exec-broker/tests/mcp_tls.rs`
- `crates/remote-exec-broker/tests/mcp_transfer.rs`
- `crates/remote-exec-broker/tests/support/*.rs`

Broker tests should stay focused on public MCP behavior, target routing,
target isolation, auth, and user-visible response shape.

Maintenance should:

- consolidate repeated transfer request construction where the exact JSON shape
  is not the thing being asserted
- share archive-inspection helpers with daemon transfer tests
- keep CLI tests that protect real command behavior or public command surface
- remove shallow CLI help-string assertions only when they are not the sole
  protection for a documented command
- leave the already-maintained port-forward and cross-target tests alone unless a
  shared helper naturally applies

### Daemon RPC Tests

Target files:

- `crates/remote-exec-daemon/tests/exec_rpc/*.rs`
- `crates/remote-exec-daemon/tests/health.rs`
- `crates/remote-exec-daemon/tests/image_rpc.rs`
- `crates/remote-exec-daemon/tests/patch_rpc.rs`
- `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- `crates/remote-exec-daemon/tests/tls.rs`
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- `crates/remote-exec-daemon/tests/windows_pty_debug.rs`
- `crates/remote-exec-daemon/tests/support/*.rs`

Daemon RPC tests should remain behavior-level tests for daemon-local contracts.

Maintenance should:

- move duplicated archive construction and inspection into shared support where
  both broker and daemon tests use it
- merge simple transfer path-info tests only when they differ solely by input and
  expected booleans
- keep patch and exec semantic coverage broad because those suites protect many
  distinct edge cases
- clean patch and exec tests with helper extraction rather than aggressive
  trimming
- clarify `windows_pty_debug.rs` as manual diagnostics or move it out of normal
  test discovery if it is not intended to participate in automated coverage

### Admin And PKI Tests

Target files:

- `crates/remote-exec-admin/tests/certs_issue.rs`
- `crates/remote-exec-admin/tests/dev_init.rs`
- `crates/remote-exec-pki/tests/ca_reuse.rs`
- `crates/remote-exec-pki/tests/dev_init_bundle.rs`

Admin and PKI tests should remain readable operator workflow examples.

Maintenance should:

- keep functional certificate and bootstrap behavior covered
- merge repeated file-existence and PEM-material assertions into local helpers
- avoid shallow command-help tests unless they are the only coverage for a public
  command name or option
- preserve the tests as examples of intended operator workflows

### C++ Daemon Tests

Target files:

- `crates/remote-exec-daemon-cpp/tests/*.cpp`
- `crates/remote-exec-daemon-cpp/Makefile` only when test-target naming or
  grouping requires it

The C++ daemon is a first-class implementation path. Its tests should not be
treated as a small appendix to Rust test work.

Maintenance should:

- keep POSIX and Windows XP-compatible constraints visible
- keep the assert-plus-Makefile harness unless a specific cleanup requires a
  small local helper file
- split or reorganize very large files only where it improves ownership and
  focused verification
- consider separating route-unit behavior from transfer, image, and exec route
  scenarios in `test_server_routes.cpp` if the result stays easy to run
- consider moving repeated tunnel/socket helper blocks out of
  `test_server_streaming.cpp` if doing so reduces the cost of maintaining v4
  tunnel coverage
- preserve focused `make test-*` targets and `make check-posix`

### Shared Test Support

Target files:

- `tests/support/transfer_archive.rs`
- broker and daemon `tests/support` modules
- optional C++ test support files if a C++ split needs them

Shared support should stay small and behavior-named.

Maintenance should:

- promote helpers only when there are at least two consumers or substantial
  duplicated setup disappears
- keep broker fixture helpers in broker test support
- keep daemon RPC fixture helpers in daemon test support
- keep archive construction and decoding helpers in `tests/support` because both
  broker and daemon transfer tests use archive semantics
- avoid a single universal support module with unrelated helpers

## Candidate Work Items

The implementation plan should decompose work into independent tasks with a
focused commit after each task. Good candidate tasks are:

1. Move duplicated tar construction and path-reading helpers into
   `tests/support/transfer_archive.rs`, then update broker and daemon transfer
   tests to use them.
2. Consolidate broker transfer helper code for common single-source local and
   remote transfer calls while leaving request-shape tests explicit.
3. Consolidate daemon transfer path-info and archive assertions where setup is
   repeated and behavior remains distinct.
4. Clean admin and PKI test helper repetition without changing the operator
   scenarios under test.
5. Clean C++ route and session-store tests by extracting local helper functions
   or splitting route concerns if focused verification remains straightforward.
6. Clean C++ streaming/tunnel tests only after identifying repeated socket and
   frame helpers that can move without obscuring the scenario bodies.
7. Clarify or relocate manual Windows PTY diagnostics so normal automated test
   discovery is truthful.

The plan should avoid editing port-forward and multi-target tests except for
shared helper adoption, because those suites were just maintained and verified.

## Verification Plan

Run focused verification after each task:

- Broker transfer or asset changes:
  - `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
  - `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture` when
    asset tests change
- Broker exec or CLI changes:
  - `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
  - `cargo test -p remote-exec-broker --test mcp_cli -- --nocapture`
- Daemon transfer changes:
  - `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
- Daemon exec, patch, image, health, TLS, or port-forward changes:
  - run the matching `cargo test -p remote-exec-daemon --test <name> --
    --nocapture`
- Admin and PKI changes:
  - `cargo test -p remote-exec-admin --test dev_init`
  - `cargo test -p remote-exec-admin --test certs_issue`
  - `cargo test -p remote-exec-pki --test ca_reuse`
  - `cargo test -p remote-exec-pki --test dev_init_bundle`
- C++ daemon changes:
  - run the focused `make -C crates/remote-exec-daemon-cpp test-*` target
  - run `make -C crates/remote-exec-daemon-cpp check-posix` after C++ test
    maintenance tasks

Final verification:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `make -C crates/remote-exec-daemon-cpp check-posix`
- `git diff --check`

## Risks

### Low Risk

- moving duplicated archive helper logic into shared test support
- consolidating local helper functions inside a test file
- merging repeated assertions that use the same setup and response
- documenting manual diagnostic tests

### Medium Risk

- splitting large C++ test files, because the Makefile target graph and XP build
  constraints need to remain intact
- merging daemon transfer tests, because import/export behavior has platform and
  archive-format subtleties
- removing shallow CLI coverage, because help text can still be a public signal
  when it is the only advertised command coverage

## Success Criteria

This maintenance pass is complete when:

- first-party test code has less duplicated helper logic
- broker public behavior is still covered through MCP-facing integration tests
- daemon-local behavior is still covered through RPC-level tests
- C++ daemon test files are easier to navigate without losing parity coverage
- generated build artifacts remain untracked
- every task has a focused commit
- the final Rust and C++ quality gates pass
