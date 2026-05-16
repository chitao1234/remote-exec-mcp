# Section 1 Cross-Language Contract Conformance Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Add shared cross-language conformance fixtures and tests so Rust/C++ wire and semantic contract drift fails in CI without collapsing the intentionally separate implementations.

**Requirements:**
- Keep Rust and C++ implementations separate; do not try to merge sandbox, path, transfer, or tunnel runtime code across languages in this pass.
- Preserve the current public MCP schema, broker-daemon RPC surface, port-tunnel wire format, transfer header names, warning codes, and error-code strings.
- Treat `remote-exec-proto` as the authoritative contract declaration for mechanical wire items, but use neutral fixtures so C++ can verify against the same truth.
- Focus on the verified section-1 risks only: port-tunnel framing, path policy semantics, sandbox decisions, and transfer contract semantics.
- Prefer fixture-driven conformance tests over broad refactors. Production code changes are in scope only where the new shared checks expose a real drift or missing edge case.
- Keep broker remote-platform logic syntax-only; section 1 is about Rust/C++ contract alignment, not widening broker-side platform semantics.

**Architecture:** The fix is a neutral contract-fixture layer under repo-root test data, consumed by both Rust and C++ tests. Mechanical wire items such as frame IDs, protocol constants, and transfer header names should be exported from the Rust-side contract into shared fixtures, so C++ validates against generated or Rust-authored neutral data rather than hand-copied expectations. Higher-level semantics such as path normalization, sandbox allow/deny decisions, and transfer edge cases should use shared scenario corpora that both language test suites execute through their own implementations.

**Verification Strategy:** Verify each batch through the public or contract-facing test seams in both languages, not only with compile checks. Use `cargo test -p remote-exec-proto`, `cargo test -p remote-exec-host`, focused daemon/broker transfer and port-forward tests, `make -C crates/remote-exec-daemon-cpp check-posix`, and `cargo fmt --all --check`. Expect fixture-driven failures to identify real Rust/C++ drift before any protocol or filesystem behavior changes.

**Assumptions / Open Questions:**
- Repo-root `tests/contracts/` is available as the neutral fixture location and will not conflict with existing tooling.
- A small Rust-side fixture exporter is acceptable for mechanical constants; if that proves awkward, the fallback is a manually maintained neutral fixture file with tighter Rust-side self-checks.
- Some sandbox and transfer scenarios may need temporary filesystem setup rather than pure JSON-only evaluation because canonicalization and symlink handling depend on real paths.
- Windows-native path comparison cases should remain explicitly platform-conditioned where the semantic source of truth is the Windows API rather than pure syntax normalization.

---

### Task 1: Establish shared contract fixtures for mechanical wire and header items

**Intent:** Introduce the neutral fixture layer and cover the highest-risk mechanical duplication first: port-tunnel frame constants/layout, tunnel protocol constants, transfer header names/defaults, and C++ server contract strings.

**Relevant files/components:**
- Likely create: `tests/contracts/port_tunnel/`
- Likely create: `tests/contracts/transfer_headers/`
- Likely create: `tests/contracts/server_contract/`
- Likely create: `crates/remote-exec-proto/tests/contract_fixtures.rs` or nearby Rust test support for loading neutral fixtures
- Likely create: `crates/remote-exec-daemon-cpp/tests/test_contract_fixtures.h`
- Likely modify: `crates/remote-exec-proto/src/port_tunnel/mod.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/transfer.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/server_contract.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`
- Existing references: `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`

**Notes / constraints:**
- The shared truth should cover frame type numeric values, preface bytes, header length, max meta/data sizes, tunnel version header/value, upgrade token, and transfer header names.
- Prefer neutral fixture encodings such as JSON plus hex/base64 payloads over ad hoc per-language hard-coded vectors.
- Rust remains the authoritative edit point for these wire constants, but the fixture layer must be language-neutral and consumed by C++ tests directly.
- Do not change the live wire format in this task; the deliverable is shared conformance coverage plus any tiny drift fixes the new coverage immediately exposes.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: Rust and C++ both validate the same tunnel/header fixtures, and any mechanical contract drift becomes a test failure.

- [ ] Confirm the exact neutral fixture shape and where the Rust-side exporter or self-check should live
- [ ] Add shared fixtures plus Rust and C++ loader helpers for mechanical contract items
- [ ] Convert existing tunnel/header tests to validate against the shared fixtures instead of only local hard-coded expectations
- [ ] Fix any exposed constant or framing drift without changing intended wire behavior
- [ ] Run focused cross-language verification
- [ ] Commit

### Task 2: Add shared path-policy and sandbox conformance corpora

**Intent:** Replace language-local edge-case assumptions with shared path and sandbox scenarios that both implementations must satisfy.

**Relevant files/components:**
- Likely create: `tests/contracts/path_policy_cases.json`
- Likely create: `tests/contracts/path_compare_cases.json`
- Likely create: `tests/contracts/sandbox_cases/`
- Likely modify: `crates/remote-exec-proto/src/path.rs`
- Likely modify: `crates/remote-exec-host/src/sandbox.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_sandbox.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/path_policy.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/path_compare.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/filesystem_sandbox.cpp`
- Existing references: `crates/remote-exec-host/src/host_path.rs`
- Existing references: `crates/remote-exec-host/src/path_compare.rs`
- Existing references: `crates/remote-exec-daemon-cpp/include/path_policy.h`
- Existing references: `crates/remote-exec-daemon-cpp/include/path_compare.h`

**Notes / constraints:**
- Separate syntax-only cases from host-comparison cases. `is_absolute`, normalization, basename, and join should share a syntax corpus; `path_equal` and `path_is_within` need a host-semantics corpus.
- Include verified risky forms: drive-letter aliases, UNC, `/c/...`, `/cygdrive/...`, mixed separators, trailing separators, and Unicode.
- Sandbox scenarios should cover allow-only, deny-only, allow+deny precedence, non-absolute rule rejection, missing-ancestor canonicalization, and symlink-sensitive containment.
- Keep Windows-native comparison behavior backed by the Windows implementation where appropriate; do not simplify to ASCII-only semantics just to make the fixtures easier.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: both language implementations pass the same path/sandbox scenarios, and any remaining divergence is explicit and justified rather than accidental.

- [ ] Confirm the shared path and sandbox scenario shapes, including which cases are syntax-only versus host-semantic
- [ ] Add Rust and C++ test coverage that executes the same corpora through each implementation
- [ ] Fix any exposed normalization, containment, or sandbox-rule drift while preserving the intended Windows-native semantics
- [ ] Re-check broker/host behaviors that depend on those primitives
- [ ] Run focused cross-language verification
- [ ] Commit

### Task 3: Add shared transfer semantic conformance and finish the section-1 sweep

**Intent:** Cover the remaining contract-level duplication in transfer handling through shared scenarios for headers, overwrite semantics, symlink behavior, traversal rejection, and warning emission.

**Relevant files/components:**
- Likely create: `tests/contracts/transfer_semantics/`
- Likely create: `tests/contracts/transfer_archives/` or a fixture format carrying small archive bodies as text-safe payloads
- Likely modify: `crates/remote-exec-host/src/transfer/archive/`
- Likely modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_tar.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming_routes.cpp`

**Notes / constraints:**
- Keep the scope at externally visible semantics: transfer header parsing/rendering, overwrite behavior, optional-default handling, symlink-mode behavior, warning codes/messages, summary-entry behavior, and traversal rejection.
- Do not try to unify the Rust and C++ tar implementations. The goal is shared conformance inputs, not shared archive code.
- If one giant fixture corpus becomes brittle, split it into protocol fixtures and semantic fixtures rather than weakening the coverage.
- Reuse the existing transfer warning and error-code names; this task should harden the current contract, not redesign it.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `cargo fmt --all --check`
- Expect: shared transfer scenarios pass in both languages, end-to-end broker tests still pass against the C++ daemon path, and section 1 closes with cross-language drift guarded by fixtures rather than convention.

- [ ] Confirm the transfer fixture split between pure protocol cases and archive-behavior cases
- [ ] Add shared transfer conformance scenarios and hook them into both Rust and C++ tests
- [ ] Fix any exposed drift in transfer header handling, overwrite/symlink semantics, or warning behavior
- [ ] Run the final section-1 verification sweep across Rust broker/daemon and the C++ daemon
- [ ] Commit
