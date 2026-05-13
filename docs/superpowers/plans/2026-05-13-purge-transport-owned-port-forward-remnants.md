# Port Forward Transport-Owned Remnant Purge Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Remove the remaining live `transport_owned` port-forward ownership remnants from the host and C++ daemon internals without changing the public forwarding contract or reconnect behavior.

**Requirements:**
- Preserve the current public `forward_ports` schema, broker-owned `forward_id` model, resumable listen-side session behavior, and current reconnect semantics.
- Preserve the current meaning of real transport failures and transport-drop handling. Terms such as HTTP transport, tunnel transport, retryable transport error, and test hooks that intentionally close the upgraded tunnel transport are not purge targets.
- Keep the current retained-session versus live-connection behavior intact:
  - retained listen-side TCP listeners and UDP binds survive transient broker-daemon transport loss within the current reconnect window
  - active TCP streams and live UDP peer flow do not become resumable as part of this cleanup
- Limit the purge to live code and live docs/comments. Historical documents under `docs/` are not implementation targets unless a later task explicitly asks for historical maintenance.
- Keep POSIX, Windows GNU, and XP-compatible C++ build paths working where the touched code is shared.

**Architecture:** Treat the modern forwarding ownership model as two categories only: retained listen-side session resources and live connection-local resources. In Rust host code, purge the remaining `transport_owned` naming and collapse duplicated read-loop helpers where the only difference is whether frames are emitted through the tunnel sender or an attached session sender. In the C++ daemon, replace `TransportOwnedStreams` and related helper names with connection-local terminology, while keeping the current `PortTunnelConnection` versus `PortTunnelSession` split and keeping `port_tunnel_transport.cpp` responsible for actual upgrade/frame-transport concerns.

**Verification Strategy:** Verify each language slice with the narrowest port-forward coverage that exercises the touched seams, then finish with a live-code scan that proves only legitimate transport terms remain. Rust host changes should use host tests plus daemon and broker forwarding coverage because the helpers are exercised through real tunnel flows. C++ daemon changes should use the existing POSIX forwarding/runtime coverage because the purge touches connection/session internals but should not alter behavior. The final sweep should use targeted `rg` scans to confirm that `transport_owned` naming is gone from live code while legitimate transport-drop and tunnel-transport terminology remains.

**Assumptions / Open Questions:**
- The final neutral naming should describe lifetime or attachment clearly, such as `connection_local`, `active`, or `live_tunnel`, rather than reintroducing another ambiguous ownership term. Confirm the narrowest consistent vocabulary during implementation.
- Broker test hooks like `force_close_port_tunnel_transport` are expected to stay because they model real transport loss, not the deprecated ownership split.
- The file name `port_tunnel_transport.cpp` is expected to stay unless execution discovers a stronger repo-wide reason to rename translation units. The purge target is ownership terminology inside the implementation, not the legitimate transport module boundary.

---

### Task 1: Purge Rust Host Transport-Owned Naming And Paired Loop Duplication

**Intent:** Remove the remaining `transport_owned` ownership terminology from the Rust host forwarding runtime and reduce the paired session-vs-transport helper duplication to the narrowest stable shape.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/types.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/mod.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`

**Notes / constraints:**
- Current live remnants are in `tcp.rs` and `udp.rs`, including `tunnel_tcp_connect_transport_owned`, `tunnel_tcp_read_loop_transport_owned`, `tunnel_udp_bind_transport_owned`, `tunnel_udp_read_loop_transport_owned`, and `TransportUdpBind`.
- Do not change the retained-session split in `SessionState`; only purge stale ownership wording and duplicate helper structure.
- Prefer a small shared helper or emitter abstraction over keeping nearly identical `*_session_owned` and `*_transport_owned` loops if the implementation stays local to `remote-exec-host`.
- Keep `attached_session` and other actual session-attachment concepts only if they still describe real current behavior after the rename pass.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Expect: host-local forwarding tests still pass after the helper and naming cleanup.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon-host forwarding behavior is unchanged.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker-visible forwarding behavior is unchanged.
- Run: `cargo check -p remote-exec-host --all-targets --all-features --target x86_64-pc-windows-gnu`
- Expect: the renamed/shared host forwarding helpers still compile for the Windows GNU target.

- [ ] Inspect the current host ownership seams and choose one neutral vocabulary for live connection-local resources
- [ ] Rename the host `transport_owned` helpers and structs without changing forwarding behavior
- [ ] Collapse the duplicated session-vs-live read-loop logic where a small local helper can replace paired ownership-specific functions
- [ ] Run focused host, daemon, broker, and Windows-target verification
- [ ] Commit

### Task 2: Purge C++ Daemon Transport-Owned Containers And Helper Names

**Intent:** Remove the remaining `TransportOwnedStreams` ownership vocabulary and related helper names from the C++ daemon while preserving the current `PortTunnelConnection` and retained-session behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_connection.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_streams.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_streams.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_spawn.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`

**Notes / constraints:**
- Current live remnants include `TransportOwnedStreams`, `transport_streams_`, `close_transport_owned_state`, and `udp_read_loop_transport_owned`.
- Keep the current runtime meaning:
  - listen mode uses `PortTunnelSession` retained resources
  - connect mode and accepted active streams remain connection-local and non-resumable
- Do not widen scope into a new port-forward architecture or try to make active TCP/UDP state resumable in this purge.
- Keep legitimate transport concepts intact:
  - `port_tunnel_transport.cpp` as the upgrade/frame transport TU
  - actual send/recv transport error handling
  - test hooks that force-close the upgraded tunnel transport

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: port-tunnel behavior still passes after the ownership-term purge.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Expect: runtime/session lifecycle behavior is unchanged.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Expect: the upgrade path and route glue still work.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the full POSIX C++ surface still passes.

- [ ] Confirm the final neutral names for the connection-local stream container and close/read helpers
- [ ] Rename `TransportOwnedStreams` and related members/helpers across the C++ tunnel implementation
- [ ] Keep the current session attachment and retained-resource behavior unchanged while simplifying ownership vocabulary
- [ ] Run focused C++ forwarding/runtime verification and then `check-posix`
- [ ] Commit

### Task 3: Final Live-Code Sweep For Ownership-Term Purge

**Intent:** Confirm that the deprecated ownership vocabulary is gone from live source while real transport terminology remains where it still describes actual network transport behavior.

**Relevant files/components:**
- Existing references: `README.md`
- Existing references: `skills/using-remote-exec-mcp/SKILL.md`
- Existing references: `crates/remote-exec-host/src/port_forward/`
- Existing references: `crates/remote-exec-daemon-cpp/src/`
- Existing references: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/`

**Notes / constraints:**
- The sweep should distinguish between stale ownership vocabulary and real transport concerns.
- These are expected to remain if still accurate after implementation:
  - transport-drop error classification in broker forwarding
  - tunnel transport close/failure tests and test hooks
  - `port_tunnel_transport.cpp` as the transport translation unit name
- If a live doc, comment, or helper still uses `transport_owned` to describe connection-local forwarding state, update it in this task.
- Historical planning and audit documents are not part of the purge acceptance criteria.

**Verification:**
- Run: `rg -n "transport_owned|transport-owned|transport owned|TransportOwned" README.md skills crates/remote-exec-host/src crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/include`
- Expect: no stale ownership-term hits remain in live code or live docs.
- Run: `cargo fmt --all --check`
- Expect: formatting is clean after the rename pass.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: the broker forwarding surface still passes on the final `HEAD`.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: Rust daemon forwarding still passes on the final `HEAD`.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the final POSIX C++ forwarding surface still passes.

- [ ] Run the live-code ownership-term scan and classify any remaining `transport` hits as legitimate transport concepts or stale purge misses
- [ ] Update any remaining live comments or helper names that still describe connection-local resources as transport-owned
- [ ] Re-run the focused final forwarding verification on the current `HEAD`
- [ ] Stop and re-plan if the sweep uncovers a broader architecture dependency instead of a narrow terminology/helper cleanup
- [ ] If the sweep is clean, finish without an extra empty commit
