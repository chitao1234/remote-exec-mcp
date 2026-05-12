# Phase E2 Split God Modules And Headers Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Finish Audit Round 4 Phase E2 by splitting the largest C++ and Rust tunnel/session compilation hot spots into clearer internal modules without changing the public tool contract.

**Requirements:**
- Cover Phase E2 findings `#1` through `#5` and `#25` from `docs/CODE_AUDIT_ROUND4.md`.
- Keep the implementation grouped into exactly three medium-sized execution batches: a C++ boundary/header pass, a C++ transport decomposition pass, and a Rust proto module split.
- Preserve public MCP behavior, broker-daemon wire compatibility, C++ daemon HTTP behavior, and current port-tunnel protocol semantics.
- Keep new C++ internal boundaries internal. Do not promote implementation-only seams from `crates/remote-exec-daemon-cpp/src/` into public `include/` headers unless they are already part of the supported embedding/build surface.
- Continue the user's plan-based execution style and commit after each real task only when that task has actual code changes; do not create empty commits.

**Architecture:** Treat the C++ work as boundary-first refactoring. First narrow the compile graph around session and port-tunnel internals by moving `LiveSession` into its own public header, relocating `session_pump` locked helpers into an internal header, and splitting `port_tunnel_internal.h` into a small set of purpose-specific internal headers. Then decompose `port_tunnel_transport.cpp` so the transport unit keeps frame IO and `PortTunnelConnection` dispatch, while sender logic, spawn helpers, and transport-owned stream storage move into their own source files behind the new internal headers. Finish by splitting `remote-exec-proto::port_tunnel` into `codec` and `meta` submodules while re-exporting the same public symbols from `remote_exec_proto::port_tunnel::*` so Rust call sites stay stable.

**Verification Strategy:** Verify each batch with the narrowest existing test targets that exercise the touched seam, then widen only to the directly affected Rust or C++ integration targets. The C++ boundary pass should be checked with the session store and server runtime test targets because it changes include ownership and session/output helpers. The C++ transport pass should be checked with the port-tunnel frame, runtime, routes, and streaming targets because it changes worker/thread and tunnel dispatch internals. The Rust proto split should start with `cargo test -p remote-exec-proto`, then widen to the daemon and broker forwarding tests that compile and exercise the shared tunnel module. If any batch changes build lists or include layout in a way that plausibly affects Windows or XP support, include the relevant compile-oriented make target during execution before claiming completion.

**Assumptions / Open Questions:**
- `port_tunnel_internal.h` is intentionally an internal header under `src/`; the split should keep that status and avoid inventing a broader public C++ abstraction layer.
- The output-rendering and response-building extraction for `session_store.cpp` should preserve the current JSON response shape and truncation behavior exactly; any cleanup beyond that is out of scope for Phase E2.
- The Rust `port_tunnel` split should preserve the existing public API through re-exports unless execution reveals a contained downstream improvement that does not expand the phase.
- The plan assumes current tests already cover the relevant session and port-tunnel behavior well enough that this phase remains structural; if execution uncovers a true behavior bug, fix it only when it is necessary to complete the structural change safely.

---

### Task 1: Save The Phase E2 Plan

**Intent:** Create the tracked plan artifact for the approved three-batch Phase E2 execution shape before implementation begins.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-12-phase-e2-split-god-modules-and-headers.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start Phase E2 code changes until the user reviews and approves this merged plan artifact.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-12-phase-e2-split-god-modules-and-headers.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged design + execution plan at the tracked path
- [ ] Check the header, goal, and approved three-batch scope against the agreed design
- [ ] Confirm the plan stays limited to Phase E2 findings `#1` through `#5` and `#25`
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: C++ Boundary And Header Pass

**Intent:** Reduce compile coupling across the C++ daemon's session and port-tunnel internals before moving larger implementation blocks between source files.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/session_store.h`
- Likely create: `crates/remote-exec-daemon-cpp/include/live_session.h`
- Likely modify: `crates/remote-exec-daemon-cpp/include/session_pump.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/session_pump_internal.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_pump.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/output_renderer.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/output_renderer.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/session_response_builder.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/session_response_builder.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_service.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_connection.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_streams.h`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Existing references: `crates/remote-exec-daemon-cpp/include/server.h`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`

**Notes / constraints:**
- Keep `LiveSession` publicly includable where needed, but keep the locked helper functions private to the implementation side.
- Do not turn the new port-tunnel headers into a second omnibus include layer; each header should own one coherent slice.
- Preserve the current `SessionStore` JSON response fields and output truncation semantics while moving rendering/builder code out of `session_store.cpp`.
- Keep POSIX, Win32, GNU make, BSD make, and NMAKE include/build paths aligned where they already support the same code path.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Expect: session store and pump behavior still pass with the new header and helper ownership.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: the runtime/integration seam still compiles and passes with the narrower headers.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Expect: route-level runtime coverage still passes after the session/port-tunnel internal split.

- [ ] Confirm the current include graph and identify the exact declarations that move into `live_session.h` and `session_pump_internal.h`
- [ ] Split `session_store` formatting and response concerns into dedicated helper units while keeping runtime behavior unchanged
- [ ] Split `port_tunnel_internal.h` into a few internal headers with clear ownership and update the C++ source includes accordingly
- [ ] Update the build lists for any new helper source files or headers that affect generated dependencies
- [ ] Run focused C++ verification for session store, runtime, and routes
- [ ] Commit with real code changes only

### Task 3: C++ Transport Decomposition

**Intent:** Break `port_tunnel_transport.cpp` into smaller transport-focused source units after the internal header boundaries are clean enough to support it.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_sender.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_spawn.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_spawn.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/port_tunnel_streams.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/NMakefile`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`

**Notes / constraints:**
- Keep `port_tunnel_transport.cpp` responsible for frame preface/frame read logic and `PortTunnelConnection` dispatch rather than moving tunnel behavior into a new abstraction layer.
- Preserve current worker-budget acquisition and release semantics exactly, especially on Win32 thread start failures and the `REMOTE_EXEC_CPP_TESTING` forced-failure path.
- The deduplicated spawn helper should collapse the repeated structure without obscuring the different loop entry points for TCP read, TCP write, and UDP read.
- Update all supported makefiles for the new translation units in the same task; do not leave one build path behind.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`
- Expect: basic frame encode/decode coverage still passes.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: server runtime tunnel behavior still passes after the source split.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Expect: route-level tunnel upgrade and runtime flows still pass.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: streaming and port-tunnel transport coverage still passes with the decomposed sources.

- [ ] Confirm the code blocks in `port_tunnel_transport.cpp` that move into sender, spawn, and streams units
- [ ] Add a shared internal spawn helper and migrate the three `spawn_*_thread` entry points to it without changing worker accounting behavior
- [ ] Move sender and transport-owned stream implementations into focused source files and trim `port_tunnel_transport.cpp` down to connection/frame dispatch responsibilities
- [ ] Update GNU make, XP make, and NMAKE source lists for the new translation units
- [ ] Run focused C++ transport verification, including the streaming target
- [ ] Commit with real code changes only

### Task 4: Rust `port_tunnel` Module Split

**Intent:** Split the shared Rust port-tunnel module into codec and metadata halves while keeping the external `remote_exec_proto::port_tunnel` surface stable for broker, host, and daemon consumers.

**Relevant files/components:**
- Likely delete/replace: `crates/remote-exec-proto/src/port_tunnel.rs`
- Likely create: `crates/remote-exec-proto/src/port_tunnel/mod.rs`
- Likely create: `crates/remote-exec-proto/src/port_tunnel/codec.rs`
- Likely create: `crates/remote-exec-proto/src/port_tunnel/meta.rs`
- Likely modify: `crates/remote-exec-proto/src/lib.rs`
- Existing references: `crates/remote-exec-daemon/src/port_forward.rs`
- Existing references: `crates/remote-exec-broker/src/daemon_client.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/`
- Existing references: `crates/remote-exec-host/src/port_forward/`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Existing references: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`

**Notes / constraints:**
- Keep the public import shape stable through re-exports from `remote_exec_proto::port_tunnel`.
- Split by responsibility, not by arbitrary line count: metadata DTOs and enums in one module, frame/codec/IO logic in the other.
- Preserve existing tests, wire constants, and helper semantics such as `Frame::is_data_plane_frame`, `data_plane_charge`, `encode_frame_meta`, and `decode_frame_meta`.
- Do not widen this task into later-phase broker or host cleanup that merely becomes easier after the proto split.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Expect: the shared proto crate passes with the module split and re-exported API.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon-side forwarding RPC coverage still compiles and passes against the split module.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding coverage still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker-to-C++ forwarding coverage still passes.

- [ ] Convert `port_tunnel.rs` into a directory module while preserving the crate's public import surface
- [ ] Move tunnel metadata DTOs and enums into `meta.rs` and frame/codec/IO logic into `codec.rs`
- [ ] Re-export the expected public symbols from `port_tunnel/mod.rs` and update only the minimal internal references needed for the new module layout
- [ ] Run focused Rust verification across proto, daemon, and broker forwarding seams
- [ ] Commit with real code changes only
