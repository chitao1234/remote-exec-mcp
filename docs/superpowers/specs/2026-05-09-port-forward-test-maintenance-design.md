# Port Forward Test Maintenance Design

## Goal

Clean up the port-forward and cross-target end-to-end test suite so it is easier to read, faster to diagnose, and still strong against the high-impact regressions found during the v4 tunnel work.

## Scope

This pass is limited to port-forward tests and cross-target end-to-end tests. It may touch shared test harness code used by those tests, but it will not rewrite unrelated exec, patch, image, transfer, certificate, or generic daemon test coverage.

Historical planning notes under `docs/` are not live contract material for this pass. The live sources of truth are the Rust/C++ tests and the current README quality gates.

## Current Problems

The test suite has accumulated coverage quickly while the v4 port tunnel protocol stabilized. The result is useful but uneven:

- `crates/remote-exec-broker/tests/multi_target.rs` is only a path wrapper around `tests/e2e/multi_target.rs`, so the canonical location of cross-target coverage is unclear.
- Port-forward integration tests repeat JSON construction, forward id extraction, listen endpoint extraction, and polling loops for status, phase, side health, drop counters, and active TCP streams.
- Some test names describe small harness mechanics rather than the behavior under protection, making failure output harder to triage.
- A few tests exercise the same public state transition through nearly identical fixtures and can be merged without losing coverage.
- Recent v4 changes made legacy frames reserved-but-unsupported; the suite should keep one clear public-path assertion for that behavior and avoid scattered legacy assumptions.
- C++ broker integration tests keep their own proxy and wait helpers. They should remain self-contained enough to run against the real C++ daemon, but repeated open/list/wait patterns should be cleaned when it is safe.

## Maintenance Strategy

Use a targeted cleanup approach, not a broad rewrite.

The pass will preserve high-value regression coverage for:

- v4 `TunnelOpen` and protocol-version visibility,
- listen-side and connect-side tunnel recovery,
- broker crash and graceful close listener cleanup,
- terminal tunnel errors releasing listeners promptly,
- TCP stream and UDP datagram limit/drop telemetry,
- C++ daemon parity for TCP, UDP, reconnect, and broker-crash cleanup,
- target isolation across sessions and file/port operations.

The pass will trim or merge tests only when the same behavior is already asserted through the same layer and failure mode. It will not delete tests solely because they are long or slow if they cover different state-machine edges.

## Test Layout

Cross-target e2e tests should live in one obvious broker integration-test module. The current wrapper arrangement will be replaced with a direct broker test module layout:

- `crates/remote-exec-broker/tests/multi_target.rs` becomes the canonical test file.
- `crates/remote-exec-broker/tests/multi_target/support.rs` holds the e2e cluster harness.
- `tests/e2e/multi_target.rs` and `tests/e2e/support/mod.rs` are removed after their contents are moved.

This keeps the coverage under the crate that builds and runs it today and removes the misleading root-level duplicate path.

## Harness Cleanup

Introduce focused test helper functions where they remove repeated behavior rather than hiding important assertions.

For broker port-forward tests:

- Add helpers for common `forward_ports` open/list/close calls.
- Add one polling helper that reads a forward entry and lets tests assert the specific condition they care about.
- Keep specialized helpers for status, phase, side health, drop counters, and active TCP stream counts when they produce clearer failure messages.
- Standardize timeout failure messages so they include the last observed forward entry.
- Replace manual JSON construction only where the fields are identical across tests; tests with intentionally odd inputs should keep explicit JSON.

For cross-target e2e tests:

- Keep helpers for opening TCP/UDP forwards on the broker fixture.
- Use shared listener rebind and forward-ready wait helpers instead of local polling variants where semantics match.
- Keep transfer and exec tests in the moved file only because they are cross-target isolation coverage; do not expand them.

For C++ broker integration tests:

- Keep real-daemon process setup inside `mcp_forward_ports_cpp.rs`.
- Clean repeated forward-open and polling code when the helper signature stays simple.
- Do not move C++ process management into the Rust broker support module in this pass; it has different lifecycle and build requirements.

## Test Additions

Add only high-signal tests that fill gaps exposed by the v4 cleanup:

- A broker/daemon public-path test that sends reserved legacy session frames to the daemon tunnel and asserts an `invalid_port_tunnel` error frame. Host unit tests already cover this internally, but the HTTP upgrade/RPC path should have one public assertion.
- A cross-target or broker integration assertion that target info still reports port forward protocol version 4 for supported Rust daemon targets. C++ already has this coverage; Rust broker/daemon coverage should remain explicit after test movement.

If existing tests already cover these exact assertions by the time implementation starts, the task should refactor the existing assertion instead of adding another test.

## Test Trimming Rules

A test can be removed or merged only if all of these are true:

- It exercises the same public API layer as another test.
- It sets up the same protocol, side roles, and failure trigger.
- It asserts the same externally visible state transition or error.
- The remaining test name and assertion messages make the protected behavior clear.

A test should stay if it differs by any of these dimensions:

- listen side vs connect side failure,
- TCP vs UDP,
- Rust daemon vs real C++ daemon,
- retryable transport loss vs terminal protocol error,
- graceful close vs broker crash vs daemon restart,
- public broker behavior vs host-local unit behavior.

## Verification

Each task will run focused tests for the files it touches. The final verification gate is:

- `cargo test -p remote-exec-broker --test mcp_forward_ports -- --nocapture`
- `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp -- --nocapture`
- `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- `cargo test -p remote-exec-daemon --test port_forward_rpc -- --nocapture`
- `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Non-Goals

- No production behavior changes.
- No broad transfer, exec, patch, image, TLS, or certificate test cleanup.
- No replacement of the current Rust test framework or C++ make harness.
- No attempt to make historical docs match the current protocol.
- No generated build artifacts in commits.
