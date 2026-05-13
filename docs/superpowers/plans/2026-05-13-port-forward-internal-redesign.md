# Port Forward Internal Redesign Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Redesign the Rust port-forward internals so broker and host both use explicit stable-session plus active-epoch boundaries, with authoritative generation and attachment rules, while preserving the public API and live wire format.

**Requirements:**
- Keep the public `forward_ports` API and the live port-tunnel wire format unchanged.
- Make broker-side forward generation authoritative for every active epoch, including reconnect and resumed listen-session flows.
- Make host-side active attachment generation authoritative for frame validation and error emission.
- Remove the current split between tunnel-owned and sender-only error helper paths.
- Preserve existing reconnect behavior and retained listen-session semantics unless the redesign explicitly tightens currently loose internal invariants.
- Allow internal file and module reorganization where it improves boundary clarity.
- Keep C++ daemon behavior and broker interoperability intact; if a shared protocol assumption is exposed, address it in the same pass or bound it explicitly during execution.

**Architecture:** The redesign introduces one stable lifetime layer and one active runtime layer on both sides. On the host, resumable listen sessions own retained resources and current attachment metadata, while per-connection active attachments own the sender, cancel scope, generation, and active TCP/UDP maps used by handlers. On the broker, forward identity stays stable, listen-session state owns resumable listen metadata, and a per-generation active epoch owns the current tunnel pair plus bridge-local runtime state. Generation is a forward-epoch concept owned by the broker and validated by the host, not an incidental field stored on whichever object currently sends frames.

**Verification Strategy:** Use focused Rust port-forward coverage after each task, then finish with the broker and daemon port-forward integration tests plus any C++ interoperability coverage that exercises broker-managed tunnels. At minimum, run `cargo test -p remote-exec-daemon --test port_forward_rpc`, `cargo test -p remote-exec-broker --test mcp_forward_ports`, and `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`; during execution, add a focused `remote-exec-host` port-forward unit-test selector after confirming the exact command shape in the crate.

**Assumptions / Open Questions:**
- Confirm during implementation whether any broker-to-C++ daemon path still assumes listen-session generation is always `1`; if so, either update that implementation in the same pass or explicitly scope the redesign to the Rust host path before merging.
- Confirm the best final module names while preserving the approved boundary model; names such as `active.rs`, `epoch.rs`, or `lifecycle.rs` are shape guides, not mandatory filenames.
- Confirm whether broker-side terminal tunnel-error reporting should retain generation only for diagnostics or also use it to harden stale-epoch filtering in recovery paths.

---

### Task 1: Split Host Session Ownership From Active Attachment Runtime

**Intent:** Refactor the host port-forward runtime so retained listen-session state and current active attachment state become explicit, authoritative, and independently testable.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/types.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify or replace: `crates/remote-exec-host/src/port_forward/access.rs`
- Likely create: `crates/remote-exec-host/src/port_forward/active.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/udp.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`

**Notes / constraints:**
- `ListenSession` must become the sole owner of retained listener / retained UDP bind / resume lifetime state.
- Active TCP stream maps, active UDP reader maps, sender access, and generation must live under the current attachment/runtime object, not on retained session state.
- Replace the tunnel-vs-sender error helper split with one explicit send path that always carries the effective generation when an attachment is active.
- Keep frame types, metadata layouts, and broker-visible behavior unchanged.

**Verification:**
- Run: `cargo test -p remote-exec-host [confirm exact port-forward unit-test selector]`
- Expect: host port-forward unit tests pass, including resume, retained-resource, and generation-mismatch coverage.

- [ ] Inspect the current host ownership seam and confirm the exact replacement shape for `TunnelState`, `SessionState`, and any new active-context types.
- [ ] Introduce the host-side stable session and active attachment split, including unified error-send helpers and authoritative attachment generation storage.
- [ ] Replace `tunnel_access(...)` with active-context resolution helpers that return the correct sender, generation, cancel scope, and active stream/bind maps for TCP and UDP handlers.
- [ ] Update host unit tests to assert the new generation and attachment invariants directly rather than via incidental state.
- [ ] Run focused host verification.
- [ ] Commit.

### Task 2: Make Broker Epoch And Generation Authority Explicit

**Intent:** Refactor the broker so forward generation, resumable listen-session state, and active tunnel-pair state become explicit and authoritative rather than spread across supervisor, reconnect, and bridge code.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/session.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/epoch.rs`
- Likely create or reshape: `crates/remote-exec-broker/src/port_forward/recovery.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/events.rs`

**Notes / constraints:**
- Remove the fixed `LISTEN_SESSION_GENERATION = 1` model and replace it with broker-owned forward epoch generation.
- Opening, resuming, and reconnecting listen or connect tunnels must all use the current authoritative epoch generation.
- A new broker active epoch should not become visible to bridge code until both sides are opened and validated for that generation.
- Preserve current public store state and reconnect semantics while tightening the internal state machine.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker port-forward flows still open, reconnect, and close correctly, with generation transitions covered by focused tests.

- [ ] Inspect the current broker supervisor and reconnect seams and confirm where stable listen-session state and active epoch state should live.
- [ ] Introduce explicit broker listen-session and active-epoch types, with broker-owned generation rotation for initial open and recovery.
- [ ] Move epoch transition logic out of bridge-local policy and into reusable open/recover/close helpers.
- [ ] Update broker tests to cover generation rotation, resumed listen-session attachment, and stale-epoch rejection paths.
- [ ] Run focused broker verification.
- [ ] Commit.

### Task 3: Migrate Bridge And Handler Flows Onto The New Boundaries

**Intent:** Finish the redesign by migrating hot-path handler and bridge logic to the new host active-context and broker active-epoch boundaries, then perform a final interoperability sweep.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`

**Notes / constraints:**
- Bridge loops should operate on explicit active-epoch objects rather than discover live state from `listen_session.current_tunnel()` plus local tunnel variables.
- Host TCP/UDP handlers and read loops should consume the same active-context abstraction instead of branching separately on connect vs listen attachment ownership.
- Broker tunnel error decoding should retain generation metadata if present, even though the wire schema does not change.
- Finish with a confirmatory sweep for dormant state, duplicated helpers, and any remaining call sites that bypass the new boundaries.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: Rust daemon, broker, and broker-to-C++ port-forward coverage all pass without wire-format regressions.

- [ ] Migrate host TCP/UDP handlers and read loops onto the active-context abstraction, removing remaining role/protocol branching that only exists to choose state ownership.
- [ ] Migrate broker TCP/UDP bridges onto the active-epoch abstraction, keeping recovery behavior but removing epoch ownership from bridge-local ad hoc state.
- [ ] Update broker tunnel error decoding and related diagnostics so generation is preserved through the internal event pipeline.
- [ ] Run the focused daemon and broker integration tests, then perform a final targeted sweep for stale helper paths and dead generation state.
- [ ] Commit.
