# Test Trimming Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `crates/remote-exec-admin/tests/dev_init.rs`
- `crates/remote-exec-admin/tests/dev_init_cli.rs`
- `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- `crates/remote-exec-broker/tests/mcp_assets.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `crates/remote-exec-daemon/tests/exec_rpc.rs`
- `crates/remote-exec-daemon/tests/image_rpc.rs`
- `crates/remote-exec-daemon/tests/patch_rpc.rs`
- `tests/e2e/multi_target.rs`

## Goal

Reduce test maintenance cost by removing clearly redundant or low-signal tests while preserving meaningful behavioral coverage for the current public tool surface.

The resulting suite should:

- contain fewer tests
- keep production code unchanged
- retain coverage for the broker public surface, daemon image resize families, and admin `dev-init` behavior
- remain easy to run through the existing workspace quality gate

## Optimization Target

This design optimizes for smaller long-term test maintenance rather than maximum diagnostic granularity.

That means:

- combining identical-setup tests when they assert the same response object
- removing shallow or weak tests when stronger nearby coverage already exists
- keeping one meaningful representative per behavior family instead of preserving every current assertion as a standalone test

It does not try to minimize runtime at all costs. The primary goal is reducing duplication and low-value maintenance burden.

## Scope

This design covers only test-file changes.

In scope:

- merging duplicate broker happy-path tests
- removing one redundant broker interception integration test
- strengthening and keeping image resize coverage at one meaningful test per format family
- removing the shallow admin CLI help smoke test

Out of scope:

- production code changes
- daemon patch test trimming
- daemon exec RPC test trimming
- end-to-end multi-target test trimming
- changes to test support utilities unless needed to support the test-file edits

## Current Suite Assessment

### Tests that should remain untouched

The following areas are not good trim targets because they cover distinct invariants rather than obvious duplication:

- `crates/remote-exec-daemon/tests/patch_rpc.rs`
  - covers patch engine semantics, partial application behavior, EOF handling, repeated-context behavior, and non-UTF8 edge cases
- `crates/remote-exec-daemon/tests/exec_rpc.rs`
  - covers login policy, environment normalization, output truncation, session behavior, and concurrency
- `tests/e2e/multi_target.rs`
  - covers cross-target routing, session isolation, and restart invalidation through the real broker-plus-daemon path

These suites are intentionally broad and should remain as-is in this trimming pass.

### Tests that are good trim candidates

The following tests have meaningful overlap or weak signal:

- broker `exec_command` happy-path tests in `crates/remote-exec-broker/tests/mcp_exec.rs`
  - one test asserts opaque `session_id`
  - another uses identical setup and asserts `session_command`
- broker `write_stdin` happy-path tests in `crates/remote-exec-broker/tests/mcp_exec.rs`
  - one test asserts routing via public session id
  - another uses identical setup and asserts preserved original command metadata
- broker alias interception test in `crates/remote-exec-broker/tests/mcp_exec.rs`
  - alias parsing already has direct unit coverage in `crates/remote-exec-broker/src/tools/exec_intercept.rs`
  - broader broker interception behavior is already covered by direct-intercept and heredoc/whitespace cases
- large-PNG resize test in `crates/remote-exec-daemon/tests/image_rpc.rs`
  - currently named as a resize test but only checks the returned MIME prefix and `detail`
  - it does not decode the returned image or assert resize bounds
- admin CLI help test in `crates/remote-exec-admin/tests/dev_init_cli.rs`
  - only checks that help text mentions certain flags
  - functional `dev-init` behavior already has direct coverage

## Decision Summary

### 1. Merge duplicated broker happy-path tests

Two pairs of broker tests should be merged:

- `exec_command` happy path:
  - opaque `session_id`
  - `session_command`
- `write_stdin` happy path:
  - correct routing by public session id
  - preserved original command metadata

Each merged test should:

- keep the same fixture setup
- make the same tool call sequence
- assert all relevant fields from the one shared response

This reduces maintenance by removing repeated setup and repeated fixture plumbing without losing coverage.

### 2. Remove the standalone alias interception integration test

The broker-level alias-only interception test should be removed.

Rationale:

- alias parsing is directly covered by unit tests in `exec_intercept.rs`
- broker interception behavior is already covered by:
  - direct intercepted `apply_patch`
  - heredoc interception with `cd`
  - whitespace-tolerant interception forms

Keeping a separate alias-only broker integration test adds maintenance cost without protecting a uniquely exposed behavior seam.

### 3. Keep one meaningful image resize representative per format family

Image resize coverage should stay present, but only as one meaningful representative per format family.

The retained families should be:

- passthrough family
  - existing small PNG/JPEG/WebP default-mode passthrough test remains
- PNG resize family
  - keep PNG resize coverage, but strengthen the test to decode the returned image and assert actual resize bounds
- JPEG resize family
  - existing large-JPEG resize test remains
- re-encode family
  - existing GIF default-mode re-encode test remains

This preserves the requested guarantee that image resize behavior remains represented across format families while replacing the current weak PNG resize coverage with a meaningful assertion.

### 4. Drop the admin CLI help smoke test

The CLI help test should be removed.

Rationale:

- it validates flag names in help output, not actual command behavior
- it is shallow string matching against help text
- the real `dev-init` workflow already has direct functional coverage in `dev_init.rs`

For a maintenance-first trim, this is acceptable to lose.

## Detailed Change List

### Broker tests

Modify `crates/remote-exec-broker/tests/mcp_exec.rs`:

- merge the current `exec_command` opaque-session-id test and session-command test into one test
- merge the current `write_stdin` routing test and preserved-command-metadata test into one test
- delete the standalone alias interception test

Keep the following broker tests unchanged:

- direct intercepted `apply_patch`
- raw patch not intercepted
- heredoc interception with `cd`
- whitespace-tolerant interception forms
- intercepted failure path
- warning metadata cases
- unknown-session and retryable-error cases
- broker availability and identity-verification cases

### Daemon image tests

Modify `crates/remote-exec-daemon/tests/image_rpc.rs`:

- replace the current weak large-PNG resize assertions with decoded-image assertions
- keep the existing JPEG resize test
- keep the passthrough and GIF re-encode tests

The strengthened PNG resize test should assert at least:

- returned MIME type stays `image/png`
- decoded output dimensions are reduced to the expected max bounds for default mode
- `detail` remains `None`

### Admin tests

Remove `crates/remote-exec-admin/tests/dev_init_cli.rs`.

No replacement test is required in this trimming pass.

## Risks

### Low risk

- merging duplicate broker happy-path tests
- removing the CLI help smoke test
- strengthening the PNG resize test

### Low to medium risk

- removing the alias interception integration test

This is acceptable because alias parsing still remains explicitly covered in unit tests and other broker interception tests still prove the end-to-end interception path.

## Verification Plan

Run focused suites first:

- `cargo test -p remote-exec-broker --test mcp_exec`
- `cargo test -p remote-exec-broker --test mcp_assets`
- `cargo test -p remote-exec-daemon --test image_rpc`
- `cargo test -p remote-exec-admin --test dev_init`

Then run the workspace quality gate:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Success Criteria

This design is complete when:

- the test count is lower than today
- no production files are modified
- the broker test suite still covers:
  - opaque public session ids
  - preserved original command metadata
  - intercepted patch behavior
  - warning metadata behavior
- the daemon image suite still covers:
  - passthrough behavior
  - PNG resize behavior
  - JPEG resize behavior
  - GIF re-encode behavior
- admin functional `dev-init` coverage remains
- the workspace quality gate passes
