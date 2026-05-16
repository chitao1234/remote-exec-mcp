# Section 7 Long Function And Nesting Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Keep commits non-empty and verify each task before committing.

**Goal:** Reduce the verified long-function and deep-nesting hotspots from section 7 by extracting coherent helper seams that improve readability without changing broker, daemon, or host behavior.

**Requirements:**
- Limit this pass to the validated section-7 items from `docs/code-quality-audit-2026-05-16.md`.
- Keep behavior unchanged: no timeout, recovery, shell-selection, or port-forward policy changes.
- Prefer extraction at already-visible responsibility seams rather than broad file splits or new abstraction layers.
- Preserve current logging, error codes, and recovery paths exactly unless a verification-driven adjustment is required.

**Validated scope:**
- Accept the host TCP accept-loop cleanup in `crates/remote-exec-host/src/port_forward/tcp.rs`.
- Accept the broker TCP accept pairing cleanup in `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`.
- Accept the broker forward-open orchestration cleanup in `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`.
- Accept the Windows shell default-resolution flattening in `crates/remote-exec-host/src/exec/shell/windows.rs`.

**Architecture note:** The right fix shape here is extraction by lifecycle phase:
- host TCP accept loop: separate accept/error handling, permit acquisition, and per-stream attachment setup;
- broker TCP bridge: separate listener-side backpressure / stream-id allocation / connect-open send from the state mutation that records a paired stream;
- open supervisor: split “send listen-open and wait for ready” from “register runtime and record”;
- Windows shell resolution: turn the nested fallback ladder into candidate iteration while keeping the same probe order.

**Verification strategy:** Use focused tests around the exact touched exec and port-forward seams plus formatting checks at the end.

---

### Task 1: Flatten host and broker TCP accept paths

**Intent:** Reduce nesting in the two accept-side TCP hot paths without changing the stream lifecycle they implement.

**Relevant files/components:**
- `crates/remote-exec-host/src/port_forward/tcp.rs`
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`

**Notes / constraints:**
- In `tunnel_tcp_accept_loop`, prefer extracting:
  - accepted-stream acquisition and early error/report handling
  - active-stream permit acquisition
  - per-stream registration / `TcpAccept` frame emission
- In `handle_listen_tcp_accept`, prefer extracting:
  - stream-id allocation / exhaustion recovery
  - outbound `TcpConnect` open send with retryable-transport handling
  - paired-stream state insertion once setup succeeds
- Do not change recovery semantics when the connect side is lost or the active-stream limit is exceeded.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`

- [ ] Extract the host TCP accept loop into smaller helper steps
- [ ] Extract the broker TCP accept pairing path into smaller helper steps
- [ ] Verify port-forward behavior remains unchanged
- [ ] Commit

### Task 2: Split broker open-forward orchestration at the listener-open boundary

**Intent:** Make `build_opened_forward` easier to follow by separating listener-open/ack handling from record/runtime assembly.

**Relevant files/components:**
- `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`

**Notes / constraints:**
- Keep the existing timeout, abort-on-open-failure, generation, and record-construction behavior intact.
- Prefer one helper for “send listen-open and wait for ready endpoint” and one helper for “construct runtime/record from opened tunnels.”
- Avoid moving this into another file unless the helper surface clearly needs shared reuse.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`

- [ ] Extract the listener-open and acknowledgement wait path from `build_opened_forward`
- [ ] Leave runtime/record construction as a separate, shorter step
- [ ] Verify forward open behavior remains unchanged
- [ ] Commit

### Task 3: Flatten Windows default-shell resolution and run the final sweep

**Intent:** Replace the nested fallback ladder in the Windows shell resolver with a flatter candidate-driven flow, then finish the section with the verification sweep.

**Relevant files/components:**
- `crates/remote-exec-host/src/exec/shell/windows.rs`

**Notes / constraints:**
- Preserve the current precedence:
  1. configured default shell
  2. Git Bash autodetect
  3. `pwsh.exe`, `powershell.exe`, `powershell`
  4. `COMSPEC`
  5. `cmd.exe`
- Preserve the current probing/validation behavior and error wording unless implementation needs only minimal wording adjustment to support the flatter flow.
- This task also includes the final section-7 sweep.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Run: `cargo fmt --all --check`

- [ ] Flatten the Windows default-shell candidate resolution while preserving current probe order
- [ ] Review the section-7 diff for accidental policy changes
- [ ] Run the final verification sweep
- [ ] Commit
