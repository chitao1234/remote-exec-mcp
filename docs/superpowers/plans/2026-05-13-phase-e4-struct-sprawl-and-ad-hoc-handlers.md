# Phase E4 Struct Sprawl And Ad-Hoc Handlers Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Finish Audit Round 4 Phase E4 by collapsing broker port-forward runtime/flag sprawl, replacing remaining C++ ad-hoc exec and tunnel metadata parsing with typed request helpers, and tightening the lingering broker runtime/tool seams without changing the public tool contract.

**Requirements:**
- Cover the full intended Phase E4 range from `docs/CODE_AUDIT_ROUND4.md`: `#32` through `#46`.
- Resolve the audit-summary overlap explicitly: even though the later “Phase E5” shorthand re-lists `#39`, `#41`, `#42`, and `#43`, keep them in this Phase E4 plan because they naturally fold into adjacent medium-sized batches and should not be split into separate cleanup-only follow-ups.
- Keep the implementation grouped into exactly three medium-sized execution batches: a broker port-forward runtime shape pass, a C++ typed-request and metadata pass, and a broker runtime/tool hygiene pass.
- Preserve public MCP behavior, broker CLI behavior, streamable-HTTP behavior, broker-daemon wire compatibility, current port-forward protocol semantics, and current daemon HTTP error codes/messages unless a touched test already documents a different intended behavior.
- Keep the work plan-based and commit after each real task only when that task has actual code changes; do not create empty commits.
- Do not widen this phase into Phase E5 smaller cleanups outside `#32` through `#46`, even when touched files make later items look nearby.

**Architecture:** Execute Phase E4 in three batches. First, shrink the broker’s forwarding runtime surface by factoring limit scalars out of `ForwardRuntime`, deleting the redundant `ForwardRuntimeParts` builder bag, and centralizing repeated TCP-pair teardown and listen-generation handling so bridge code stops open-coding the same lifecycle steps. Second, replace the remaining C++ ad-hoc JSON field extraction in exec routes and tunnel close handling with typed request/metadata helpers, and name the remaining config loader literals so handler parsing and daemon config defaults both become self-describing. Third, clean up the remaining broker runtime/tool seams by reusing the existing local embedded-host config path for local port forwarding, removing `write_stdin`’s logging/serialization round trip and double-wrapped errors, and collapsing duplicate broker startup validation while absorbing the small transfer/list-target/daemon-client cleanup items that touch the same files.

**Verification Strategy:** Verify each batch with the narrowest existing targets that exercise the touched seam, then widen only where the refactor crosses runtime boundaries. The forwarding-runtime batch should start with broker unit coverage in `port_forward::tcp_bridge` and `port_forward::udp_bridge`, then run at least one broker integration path against both Rust-host and real C++ daemon forwarding. The C++ typed-request batch should rely on the daemon’s route, runtime, frame, and config test targets because the refactor changes request parsing, error mapping, and default configuration naming in one runtime. The broker hygiene batch should use targeted exec, transfer, assets/list-targets, HTTP startup, and forwarding tests because it touches request logging, local port runtime creation, transfer option plumbing, daemon error decoding, and startup state construction.

**Assumptions / Open Questions:**
- For `ListenSessionControl.generation` (`#36`), the lower-risk direction is to remove the false “always 1” state and leave explicit reconnect-generation TODO markers unless implementation reveals a small contained rotation path that does not widen behavior.
- For `ForwardRuntime` (`#32`/`#33`), prefer a local `ForwardLimits` sub-struct over duplicating more summary/builder layers; the end state should reduce call-site construction burden, not just rename the bag.
- For the C++ exec-route cleanup (`#37`), preserve the existing `bad_request`, `invalid_pty_size`, `tty_unsupported`, and related wire error strings unless a targeted route test proves the current behavior is already inconsistent.
- For `startup::build_state` (`#46`), prefer making validation single-source via a `ValidatedBrokerConfig`-style seam or equivalent ownership shift rather than leaving `load()` and `build_state()` both normalize/validate the same values.

---

### Task 1: Save The Phase E4 Plan

**Intent:** Create the tracked plan artifact for the approved three-batch Phase E4 execution shape before implementation begins.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-13-phase-e4-struct-sprawl-and-ad-hoc-handlers.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start Phase E4 code changes until the user reviews and approves this merged plan artifact.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-13-phase-e4-struct-sprawl-and-ad-hoc-handlers.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged design + execution plan at the tracked path
- [ ] Check the header, goal, and approved three-batch scope against the agreed design
- [ ] Confirm the plan keeps the full intended Phase E4 range `#32` through `#46`
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: Broker Port-Forward Runtime Shape Pass

**Intent:** Reduce the broker’s forwarding-runtime sprawl by collapsing redundant runtime construction layers, extracting limit fields into a coherent sub-struct, and centralizing repeated TCP-pair teardown/lifecycle handling.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/test_support.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Cover findings `#32`, `#33`, `#34`, `#35`, and `#36`.
- `ForwardRuntime` should end with a clearer ownership split between identity/routing state and limit values. Avoid replacing one large flat bag with another differently named flat bag.
- Remove `ForwardRuntimeParts` only after the replacement construction path keeps supervisor call sites and tests simpler than the current state.
- Centralize the repeated “remove stream entry + release pending budget + release active stream” teardown so both error and pressure paths stop open-coding it.
- Preserve reconnect semantics, active-stream accounting, pending-byte accounting, and drop counters exactly. This task is structural, not a behavior rewrite.
- If `generation` is removed rather than made real, leave the two reconnect/open call sites explicit and documented instead of silently hardcoding `1` deeper in the stack.

**Verification:**
- Run: `cargo test -p remote-exec-broker --lib port_forward::tcp_bridge`
- Expect: TCP bridge lifecycle and teardown unit coverage still passes with the shared helpers.
- Run: `cargo test -p remote-exec-broker --lib port_forward::udp_bridge`
- Expect: UDP bridge/runtime unit coverage still passes with the reshaped runtime.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding behavior still passes against the Rust-host path.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker forwarding behavior still passes against the real C++ daemon path.

- [ ] Confirm the current `ForwardRuntime`/`ForwardRuntimeParts` construction graph and choose the final `ForwardLimits` ownership shape
- [ ] Refactor supervisor/runtime construction to remove the redundant intermediate bag and reduce inline limit-field repetition
- [ ] Replace the duplicated TCP pair close/remove/release sequences with shared helpers that preserve current accounting
- [ ] Resolve `ListenSessionControl.generation` by either making the state meaningful or removing the false abstraction with explicit TODO markers
- [ ] Run focused broker forwarding verification across unit and integration seams
- [ ] Commit with real code changes only

### Task 3: C++ Typed Exec/Tunnel Requests And Named Config Defaults

**Intent:** Remove the remaining ad-hoc JSON field extraction in the C++ daemon’s exec routes and tunnel-close path while naming the last bare config loader literals so the request/config code is typed and self-describing.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_route_exec.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/include/server_request_utils.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/server_route_common.h`
- Existing references: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`

**Notes / constraints:**
- Cover findings `#37`, `#38`, and `#39`.
- Prefer one typed parse at handler entry for exec start/write, analogous to the existing transfer metadata parsing style, instead of scattering repeated `body.at(...).get<...>()` lookups through each route.
- Centralize `Json::exception` to wire-error mapping where practical, but do not broaden the C++ route architecture beyond the exec/tunnel parsing needs in this phase.
- `parse_tunnel_close_metadata` may stay local to `port_tunnel_transport.cpp` if that keeps the boundary smaller; do not create a new public header unless more than one translation unit needs it.
- Promote the `max_request_header_bytes`, `max_request_body_bytes`, and `max_open_sessions` defaults to named constants next to the existing `DEFAULT_PORT_FORWARD_*` config constants so tests and readers point at one canonical location.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Expect: route parsing and RPC error mapping still pass with typed exec request parsing.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: server runtime and health/exec behavior still pass with the new route helpers and tunnel-close parser.
- Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`
- Expect: port-tunnel frame/metadata handling still passes after the close-metadata cleanup.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
- Expect: config default naming and loader behavior still pass with the new constants.

- [ ] Identify the repeated exec route fields and define typed request structs/helpers that parse them once per handler entry
- [ ] Replace inline tunnel-close metadata parsing with a typed helper that preserves current wire errors
- [ ] Promote the remaining bare config loader defaults into named constants and update config tests to reference the canonical names
- [ ] Run focused C++ route, runtime, frame, and config verification
- [ ] Commit with real code changes only

### Task 4: Broker Runtime/Tool Hygiene Pass

**Intent:** Finish the remaining broker-side structural cleanup by reusing the local embedded-host config path, removing `write_stdin`’s error/logging anti-patterns, eliminating redundant startup validation, and absorbing the adjacent small transfer/daemon-client/list-targets cleanup items in the same touched files.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/local_port_backend.rs`
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely modify: `crates/remote-exec-broker/src/startup.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_http.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Cover findings `#40`, `#41`, `#42`, `#43`, `#44`, `#45`, and `#46`.
- Reuse the existing `LocalTargetConfig::embedded_host_config` path for local port forwarding if possible; if a smaller extracted config seam is cleaner, keep one authoritative source of local embedded-host defaults rather than two parallel inline builders.
- `write_stdin_inner` should return enough typed information for completion logging without re-parsing its own structured JSON. Keep the public tool output shape unchanged.
- Preserve the existing user-facing `write_stdin failed: ...` text contract where tests assert it, while avoiding internal double-wrapping that discards the original error chain before formatting.
- Make the `decode_rpc_error` / `decode_rpc_error_strict` split self-explanatory by either collapsing them into a parameterized helper or documenting the different body-read failure policy at the call sites that need it.
- `list_targets` should gain consistent completion timing without inventing target context where none exists; a brief comment explaining the absence of `set_current_target` is preferable to future “fixes” that add misleading context.
- `build_state` should stop repeating `load()`’s normalization/validation work while keeping manual-test and unit-test construction paths clear.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: exec start/write flows, session routing, and malformed-response handling still pass with the `write_stdin` logging/result cleanup.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: transfer flows still pass with the single `TransferExecutionOptions` construction path.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: target listing and cached daemon metadata paths still pass with the list-targets/startup cleanup.
- Run: `cargo test -p remote-exec-broker --test mcp_http`
- Expect: streamable-HTTP startup/build-state behavior still passes with the validation ownership change.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: local port runtime creation still works for forwarding flows after reusing the embedded-host config path.

- [ ] Replace the local port runtime’s ad-hoc embedded-host config construction with the broker’s canonical local-host config path
- [ ] Collapse the duplicated/small broker cleanups in transfer, daemon-client, and list-targets while keeping the file touch set bounded
- [ ] Restructure `write_stdin_inner` and its logging path so completion data stays typed and failures keep their original error chain
- [ ] Remove duplicate startup normalization/validation by moving to a single clear validated-config ownership seam
- [ ] Run focused broker exec, transfer, assets, HTTP, and forwarding verification
- [ ] Commit with real code changes only
