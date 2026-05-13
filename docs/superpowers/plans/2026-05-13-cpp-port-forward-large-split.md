# C++ Port Forward Large Split Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Redesign the C++ daemon port-forward listen-side internals around explicit retained-session and attachment-runtime boundaries, with exactly one retained listen-side resource per session, while preserving the current wire format and broker-daemon contract.

**Requirements:**
- Keep the live port-tunnel wire format unchanged, including frame types, metadata fields, and error envelope shape.
- Preserve broker interoperability and the public `forward_ports` behavior; this is an internal C++ daemon redesign, not a public contract change.
- Enforce one retained listen-side resource per resumed listen session: one TCP listener for TCP listen tunnels or one UDP bind for UDP listen tunnels.
- Reject a second retained open on the same listen session deterministically as protocol misuse rather than implicitly replacing or accumulating retained resources.
- Keep connect-role behavior and connection-local stream handling intact except where boundary cleanup requires shared helper changes.
- Make retained session lifetime, active attachment lifetime, and cleanup ownership explicit and authoritative.
- Keep the C++ implementation aligned with the existing C++11 / XP-capable toolchain expectations; no C++03 compatibility work is required.
- Avoid a broad Rust-shape mirror; the redesign should improve clarity and future changeability within the existing C++ daemon style.

**Architecture:** The redesign introduces one stable state owner and one active runtime owner for listen-role tunnels. A retained listen session owns resumable identity, generation, attachment state, resume timeout, accepted-stream ID allocation, and exactly one retained resource that survives detach. A session attachment owns only the currently attached transport endpoint and its live attachment-local runtime state, while `PortTunnelConnection` becomes a transport/frame handler that delegates retained-resource and attachment operations to session/service helpers instead of reaching into session internals directly.

**Verification Strategy:** Use focused C++ streaming coverage after each task, then finish with the relevant broker-to-C++ integration coverage. At minimum, run `make -C crates/remote-exec-daemon-cpp test-host-server-streaming` after each task, then finish with `make -C crates/remote-exec-daemon-cpp check-posix` and `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`. During execution, add narrower C++ test selectors if the make targets support them cleanly.

**Assumptions / Open Questions:**
- Confirm during execution whether the cleanest large-split shape is new internal headers for retained-session and attachment types or a narrower reorganization inside the existing `port_tunnel_*` files; the boundary model is mandatory even if filenames differ.
- Confirm whether a dedicated retained-resource tagged type is clearer than separate optional TCP-listener and UDP-bind fields, while still preserving the one-resource-per-session rule.
- Confirm the final error code and text for “second retained open on same session” during implementation; the current preferred direction is to reuse `invalid_port_tunnel` with a more specific message rather than invent a new protocol code.
- Confirm whether any direct low-level C++ tests should remain multi-tunnel but single-session, or whether separate sessions are the clearer permanent shape for retained-limit and worker-limit coverage.

---

### Task 1: Introduce Explicit Retained Session And Attachment Runtime Types

**Intent:** Replace the current generic retained-resource bag with an explicit retained listen-session model plus a separate attachment-runtime model, and consolidate detach/close/expiry cleanup around those boundaries.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_streams.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_service.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_connection.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/[confirm retained-session header/cpp split]`
- Likely create: `crates/remote-exec-daemon-cpp/src/[confirm attachment-runtime header/cpp split]`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`

**Notes / constraints:**
- The retained session must become the sole owner of resumable listen-side state, including generation, attachment pointer, resume deadline, and the one retained resource.
- The attachment-runtime object must own only the currently attached transport state and attachment-local runtime maps.
- Close, detach, and expiry should converge on shared cleanup helpers instead of the current duplicated retained-resource draining logic.
- Do not widen this task into hot-path TCP/UDP loop changes beyond what is needed to establish the new state owners.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: streaming tests still pass for open, close, detach, resume, and expiry flows after the state split.

- [ ] Inspect the current session, service, and connection ownership seams and confirm the exact retained-session and attachment-runtime type placement.
- [ ] Introduce explicit retained-session and attachment-runtime types and move ownership fields to their authoritative home.
- [ ] Consolidate close, detach, and expiry cleanup around shared retained-session lifecycle helpers.
- [ ] Update or add C++ streaming coverage that directly asserts the new detach/resume and retained cleanup invariants.
- [ ] Run focused C++ streaming verification.
- [ ] Commit.

### Task 2: Make Retained Resource Ownership Singular And Session-Authoritative

**Intent:** Move listen-side retained resource registration, lookup, and close semantics behind retained-session helpers, and enforce the one-retained-resource-per-session rule for TCP listen and UDP listen tunnels.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_service.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`

**Notes / constraints:**
- A second `TcpListen` or `UdpBind` on the same resumed listen session must fail before installing resources or consuming worker threads for that retained resource.
- `Close(stream_id_of_retained_resource)` must explicitly mean “close the retained session”, not “remove one entry from a generic map that incidentally causes session close”.
- Connect-role local stream and bind behavior should remain connection-local.
- Tests that previously exercised multiple retained resources inside one session should be rewritten to use separate sessions when they are really testing limits, not intra-session fan-out.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: low-level C++ tunnel tests pass with the stricter single-retained-resource rule and updated limit/error expectations.

- [ ] Replace retained-resource maps and ad hoc lookup logic with retained-session helper operations for install, lookup, and close.
- [ ] Enforce the one-retained-resource-per-session invariant for listen-role TCP and UDP sessions with deterministic protocol errors.
- [ ] Rewrite affected C++ streaming tests to cover invalid second retained open, separate-session limit cases, and retained-close semantics explicitly.
- [ ] Run focused C++ streaming verification.
- [ ] Commit.

### Task 3: Rework Retained Worker Loops Around Attachment Snapshots And Finish Interop Sweep

**Intent:** Make retained TCP/UDP worker loops consume retained-session plus current-attachment helpers rather than raw connection ownership checks, then finish with a broker-facing confirmatory sweep.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_spawn.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_connection.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Retained worker loops should wait on the retained session for an attachment snapshot and treat that attachment as the current send/ownership authority.
- `PortTunnelConnection` should remain the owner of connect-role local streams and plain frame I/O, but should stop being the de facto owner of listen-session retained state decisions.
- Preserve current resume behavior: retained listener or bind survives transport loss, but live accepted TCP streams and attachment-local UDP readers do not.
- End with a confirmatory sweep for dead state, duplicated helpers, and remaining direct session-field access that violates the new boundary model.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: C++ streaming coverage and broker-to-C++ forwarding coverage both pass without wire-format regressions.

- [ ] Rework retained TCP accept and retained UDP read loops to operate on retained-session and current-attachment helpers instead of raw connection ownership checks.
- [ ] Narrow `PortTunnelConnection` to transport/frame and connect-local responsibilities, removing remaining direct retained-session manipulation where the new boundary provides a helper.
- [ ] Run the focused C++ and broker-to-C++ verification targets, then perform a final sweep for stale generic retained-resource assumptions.
- [ ] Commit.
