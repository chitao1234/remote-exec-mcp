# Section 2 Oversized Files Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Split the still-verified oversized and mixed-responsibility files from audit section 2 into clearer module boundaries without changing public behavior, wire format, or CLI semantics.

**Requirements:**
- Limit this pass to the section-2 items that still survive verification in the current tree.
- Keep public tool arguments, result schemas, RPC contracts, CLI flags, and broker/daemon behavior unchanged.
- Prefer medium-sized module splits along already-visible seams rather than large structural redesigns.
- Treat stale audit claims as rejected: do not split files just because the historical audit said they were too large if the current code has already shrunk or the cited problem no longer exists.
- Keep C++ section-2 work out of this pass unless a concrete, already-verified split seam becomes unavoidable during implementation.

**Architecture:** The best execution shape is by ownership boundary. First, split broker implementation files where feature seams are already explicit: daemon-client transfer logic and the `remote-exec` CLI argument/input-building surface. Second, split proto files by feature area so transfer-specific parsing/header logic and forward-port public schema stop living in large flat modules. Third, handle remaining low-risk test-layout and repetition cleanup, plus any narrowly justified production split that still reads as oversized after the first two batches.

**Verification Strategy:** Use focused broker and daemon tests around each touched public seam, plus `cargo fmt --all --check` at the end. Prefer verification that exercises the split code through public behavior rather than relying only on `cargo check`.

**Assumptions / Open Questions:**
- `daemon_client.rs` can be split without introducing a new trait or changing the `DaemonClient` public type.
- `bin/remote_exec.rs` can move builder/parser helpers under a `cli/` module tree while keeping the binary entrypoint and flag names stable.
- `rpc/transfer.rs` and `public.rs` can split by feature area while preserving re-exports expected by downstream crates and tests.
- `broker/config.rs` is no longer a priority split target because the file has already shrunk substantially; implementation should leave it alone unless a batch already touching nearby code exposes a very small obvious extraction.
- `port_forward/tcp_bridge.rs` is no longer a test-bloat problem because its tests have already moved; any remaining split there should be justified by production-code cohesion, not stale line counts.

---

### Task 1: Split broker daemon-client transfer logic and CLI helper surface

**Intent:** Reduce two high-churn broker files by moving clearly separate concerns into submodules without changing the broker API or CLI behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Likely create: `crates/remote-exec-broker/src/daemon_client/transfer.rs`
- Likely create: `crates/remote-exec-broker/src/daemon_client/mod.rs`
- Likely modify: `crates/remote-exec-broker/src/bin/remote_exec.rs`
- Likely create: `crates/remote-exec-broker/src/cli/mod.rs`
- Likely create: `crates/remote-exec-broker/src/cli/inputs.rs`
- Likely create: `crates/remote-exec-broker/src/cli/endpoints.rs`
- Existing references: `crates/remote-exec-broker/src/client.rs`
- Existing references: `crates/remote-exec-broker/src/tools/transfer/*`

**Notes / constraints:**
- In `daemon_client`, isolate transfer export/import request construction and body/stream handling from the shared HTTP/RPC transport scaffolding and typed error logic.
- In the CLI, keep the actual `main`-style command tree in the binary, but move per-tool input builders, endpoint parsing, and repetitive emit helpers into internal modules under `src/cli/`.
- Do not change flag names, CLI output shape, exit codes, or connection-mode behavior.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: transfer behavior, exec behavior, and CLI parsing/output remain identical after the module splits.

- [ ] Inspect the exact daemon-client transfer seam and CLI helper seam before moving code
- [ ] Split `daemon_client` into transport/error core plus transfer-specific helpers
- [ ] Split `remote_exec` CLI helper logic into internal modules while preserving the binary surface
- [ ] Run focused broker verification
- [ ] Commit

### Task 2: Split proto transfer and public-schema modules by feature area

**Intent:** Break up the large proto files that currently mix multiple feature areas into clearer feature-scoped modules while preserving the same exported types and functions.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/rpc/transfer.rs`
- Likely create: `crates/remote-exec-proto/src/rpc/transfer_headers.rs`
- Likely create: `crates/remote-exec-proto/src/rpc/transfer_metadata.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/mod.rs` or equivalent module wiring
- Likely modify: `crates/remote-exec-proto/src/public.rs`
- Likely create: `crates/remote-exec-proto/src/public/exec.rs`
- Likely create: `crates/remote-exec-proto/src/public/transfer.rs`
- Likely create: `crates/remote-exec-proto/src/public/forward_ports.rs`
- Existing references: `crates/remote-exec-broker/src/tools/*`
- Existing references: `crates/remote-exec-broker/src/client.rs`
- Existing references: `crates/remote-exec-daemon/src/*`

**Notes / constraints:**
- Preserve the exported wire constants, parsing helpers, and schema type names that existing crates import today; use re-exports if necessary.
- In `rpc/transfer.rs`, the most natural seam is header constants/lookup types versus metadata parse/encode helpers versus response/warning types.
- In `public.rs`, split by tool area rather than by arbitrary size: exec, transfer, patch/image, and forward-ports are the obvious groups.
- Do not let this task mutate the public schema itself; this is module layout cleanup only.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: downstream crates keep compiling against the same proto exports and transfer/forward-port behavior stays unchanged.

- [ ] Confirm the exact feature-area split and required re-exports for proto callers
- [ ] Split transfer RPC helpers into smaller internal modules
- [ ] Split public MCP schema types into feature-scoped modules with stable exports
- [ ] Run focused proto-consumer verification
- [ ] Commit

### Task 3: Clean up remaining section-2 low-risk test-layout and repetition issues, then run the final sweep

**Intent:** Finish the section with the remaining low-risk layout issues that are still worth doing after the main module splits, without reopening stale or speculative oversized-file claims.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely move: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`
- Likely create: `crates/remote-exec-host/tests/port_tunnel_tests.rs` or smaller integration-test files
- Likely modify: `crates/remote-exec-daemon/src/config/tests.rs`
- Possible narrow follow-up: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`

**Notes / constraints:**
- Treat `port_tunnel_tests.rs` as a test-layout cleanup only; do not change host port-forward production behavior in the same step unless a tiny visibility adjustment is strictly needed.
- In daemon config tests, prefer helper consolidation or table-driven reduction over changing config semantics.
- Only touch `tcp_bridge.rs` again if, after the earlier batches, there is still a clean production-only submodule seam that materially improves readability. Do not split it just to satisfy the old audit wording.
- Do not include the stale `broker/config.rs` split in this pass.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-daemon --test health`
- Run: `cargo fmt --all --check`
- Expect: test relocation/reduction does not change production behavior and the remaining section-2 cleanup stays within scope.

- [ ] Move or split the large host port-tunnel test file out of `src/`
- [ ] Reduce repetition in daemon config tests without changing coverage intent
- [ ] Review whether any remaining `tcp_bridge` split is still justified after earlier batches
- [ ] Run the final section-2 verification sweep
- [ ] Commit
