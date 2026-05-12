# Phase E3 Boundaries And Lifecycle Wrappers Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Finish Audit Round 4 Phase E3 by tightening leaky Rust crate boundaries, removing lifecycle wrapper dead weight, and cleaning up test-only broker seams without changing the public tool contract.

**Requirements:**
- Cover the remaining live Phase E3 findings from `docs/CODE_AUDIT_ROUND4.md`: `#22`, `#23`, `#24`, `#26`, `#27`, `#28`, `#29`, and `#30`.
- Treat finding `#25` as already completed by Phase E2 (`refactor: split rust port tunnel proto module`) and do not reopen it here.
- Preserve public MCP behavior, broker CLI behavior, daemon HTTP behavior, broker target metadata behavior, and current wire formats for exec, transfer, and port-forward operations.
- Keep the work plan-based and commit after each real task only when that task has actual code changes; do not create empty commits.
- Do not widen this phase into Phase E4 or E5 refactors except for absorbing `#27`, which touches the same broker boundary cleanup as `#22` and `#26`.

**Architecture:** Execute Phase E3 as three medium-sized Rust batches. First, narrow the broker library surface and move test-only streamable-HTTP address publication out of the production serving path while keeping the broker binary and CLI working. Second, simplify lifecycle identifier handling and shared RPC warning ownership so the type and module boundaries either provide real value or disappear. Third, remove daemon config wrappers and pass-through hooks that add no behavior, leaving a smaller config boundary around `DaemonConfig` and `EmbeddedHostConfig`.

**Verification Strategy:** Verify each batch with the narrowest existing tests that exercise the touched seam, then widen only where the boundary change crosses crates. Broker-boundary work should use the CLI and streamable-HTTP tests, plus at least one spawned-broker integration path that currently depends on the bound-address side channel. ID and RPC warning cleanup should start with `remote-exec-proto`, then widen to daemon exec/transfer RPC tests and broker exec/transfer tests because those are the consumers that compile and serialize the affected types. Daemon config wrapper cleanup should run the daemon config unit tests and one runtime-oriented daemon test target to confirm load/validate behavior and embedded-host conversions stay aligned.

**Assumptions / Open Questions:**
- The audit's staged summary overlaps slightly with later small-cleanup grouping; this plan keeps `#27` inside Phase E3 because it is mechanically coupled to the broker boundary pass, not because it needs its own phase.
- The best replacement for `REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE` should be confirmed during execution, but the end state must remove test env/file behavior from the production `serve_streamable_http` path itself.
- For `host::ids`, the lower-risk direction is to remove wrapper types and keep free functions returning `String`, unless execution uncovers a contained and clearly superior typed-ID plumbing path that does not widen public contracts.
- Shared RPC warning codes should move to a neutral RPC-owned seam instead of remaining inside `rpc::exec`; exact file placement (`rpc.rs` root vs a dedicated `rpc/warning.rs`) should be confirmed during implementation.

---

### Task 1: Tighten Broker Public Surface And Test-Only HTTP Seams

**Intent:** Shrink `remote-exec-broker`'s exported module surface to the actual library contract while removing streamable-HTTP test infrastructure from the production serve path and keeping the existing binaries and integration tests working.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/lib.rs`
- Likely modify: `crates/remote-exec-broker/src/main.rs`
- Likely modify: `crates/remote-exec-broker/src/bin/remote_exec.rs`
- Likely modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/generation.rs`
- Likely modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Likely modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Likely create: `crates/remote-exec-broker/tests/support/[confirm streamable-http broker harness helper]`
- Existing references: `crates/remote-exec-broker/tests/mcp_http.rs`

**Notes / constraints:**
- Keep the library entrypoints needed by the broker binary, the `remote_exec` CLI binary, and integration tests available, but prefer targeted root re-exports over leaving whole internal modules `pub`.
- The end state for `#26` must remove environment-driven file writes from `mcp_server`'s production streamable-HTTP serve path.
- `StreamIdAllocator::set_next_for_test` should remain available to tests but not to production builds.
- Preserve current streamable-HTTP listen semantics, request routing, and broker shutdown behavior.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: the public client/config/logging surface still supports the CLI binary and its integration coverage.
- Run: `cargo test -p remote-exec-broker --test mcp_http`
- Expect: streamable-HTTP startup, listen address discovery, and MCP transport behavior still pass without production-path test hooks.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: spawned broker flows that currently depend on dynamic listen address publication still pass through the replacement harness seam.

- [ ] Inventory the actual external `remote-exec-broker` library consumers from binaries and integration tests
- [ ] Narrow `lib.rs` visibility and add only the explicit re-exports still required by those consumers
- [ ] Remove `REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE` handling from the production HTTP serve path and move dynamic listen-address publication into test support
- [ ] Gate `StreamIdAllocator::set_next_for_test` to test builds only
- [ ] Run focused broker verification across CLI and streamable-HTTP integration paths
- [ ] Commit with real code changes only

### Task 2: Simplify Lifecycle IDs And Shared RPC Warning Ownership

**Intent:** Remove fake type safety around host-generated IDs and move shared warning codes to a neutral RPC seam so the remaining boundaries match actual ownership.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/ids.rs`
- Likely modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Likely modify: `crates/remote-exec-host/src/state.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-broker/src/session_store.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/exec.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/transfer.rs`
- Likely create: `crates/remote-exec-proto/src/rpc/[confirm shared warning module name]`

**Notes / constraints:**
- Keep all externally visible IDs serialized as `String`; this task is about internal boundary clarity, not wire-format changes.
- Prefer one consistent direction for `host::ids`. Do not leave a mixed state where wrappers still exist but new call sites use raw strings.
- `ExecWarning` and `TransferWarning` wire values and text should remain unchanged; only module ownership should move.
- Avoid widening this task into broader exec or transfer behavior cleanup beyond what is required to carry the boundary simplification through compile-time consumers.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Expect: shared RPC and proto tests pass with the warning-code ownership moved out of `rpc::exec`.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: exec RPC behavior still compiles and passes with the simplified ID generators and warning ownership.
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: transfer RPC behavior still compiles and passes with the new warning-code location.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec behavior still passes after ID-generation simplification.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker transfer behavior still passes with the shared warning-code move.

- [ ] Confirm the current `host::ids` usage graph and choose the single simplification path before editing
- [ ] Replace fake typed-ID wrappers with the chosen simpler boundary and update host and broker call sites consistently
- [ ] Move shared RPC warning codes to a neutral RPC-owned seam and update exec/transfer users without changing wire values
- [ ] Run focused proto, daemon, and broker verification for exec and transfer consumers
- [ ] Commit with real code changes only

### Task 3: Collapse Daemon Config Wrappers And Empty Hooks

**Intent:** Remove daemon config wrapper types and pass-through helpers that do not add behavior, leaving a smaller, clearer boundary between daemon config and host runtime config.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Likely modify: `crates/remote-exec-daemon/src/config/tests.rs`
- Likely modify: `crates/remote-exec-daemon/src/lib.rs`
- Existing references: `crates/remote-exec-host/src/config/mod.rs`
- Existing references: `crates/remote-exec-broker/src/local_backend.rs`

**Notes / constraints:**
- Preserve `DaemonConfig::load`, validation order, default values, and existing path-normalization semantics.
- Remove `EmbeddedDaemonConfig`, `prepare_runtime_fields`, and the daemon-local `normalize_configured_workdir` pass-through only if the replacement call paths remain equally clear to downstream readers.
- Prefer direct `From<EmbeddedHostConfig> for DaemonConfig` and direct host-config helper imports over wrapper layers that only forward arguments.
- Do not change config-file format or daemon startup behavior as part of this cleanup.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --lib config::tests`
- Expect: config load/normalize/validate unit coverage still passes with the wrapper layers removed.
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: daemon startup and health-path coverage still pass with the simplified config boundary.

- [ ] Confirm all live consumers of `EmbeddedDaemonConfig`, `prepare_runtime_fields`, and daemon-local workdir normalization
- [ ] Remove wrapper/pass-through seams and replace them with direct conversions or imports that preserve current behavior
- [ ] Update config unit tests or call sites only where needed to match the smaller boundary
- [ ] Run focused daemon config and startup verification
- [ ] Commit with real code changes only
