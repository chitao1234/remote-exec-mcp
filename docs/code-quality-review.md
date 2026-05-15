# Code Quality Review

Date: 2026-05-15

Scope: read-only review of the full `remote-exec-mcp` workspace. No production code was modified. Findings were synthesized from four parallel subsystem reviews plus local verification across the broker, shared Rust core, C++ daemon, and workspace-level structure.

## Executive Summary

The project has strong explicit architecture rules, but a large share of the implementation cost is being paid as manual synchronization. The same public contract is repeated across docs, schemas, registries, CLI wiring, broker handlers, daemon routes, test stubs, and the C++ implementation. That creates drift risk and makes changes review-heavy even when behavior is simple.

The second recurring problem is type erosion at subsystem boundaries. Several important flows start typed, then get flattened into `String` or `anyhow::Error`, and later reconstructed with downcasts, text matching, or ad hoc helper logic. That weakens exhaustiveness, makes tests more prose-sensitive, and raises the chance of silent contract divergence.

The third recurring problem is oversized, low-seam runtime code in the most concurrent areas, especially port forwarding and the C++ daemon transport layer. The code is not obviously broken, but it is harder to reason about than it needs to be, and the current structure makes safe change expensive.

## Prioritized Findings

### 1. No single authoritative public contract exists

Severity: High

Evidence: `AGENTS.md:7`, `AGENTS.md:137`, `README.md:12`, `README.md:76`, `skills/using-remote-exec-mcp/SKILL.md:13`, `crates/remote-exec-broker/src/mcp_server.rs:136`, `crates/remote-exec-broker/src/client.rs:170`, `crates/remote-exec-broker/src/tools/registry.rs:3`, `crates/remote-exec-broker/src/bin/remote_exec.rs:454`

Why this is a smell:

The repository explicitly says the live contract is spread across `README.md`, `AGENTS.md`, config examples, the skill, and `remote-exec-proto`. The broker then repeats the tool surface again in the MCP router, the direct-mode client dispatcher, the tool registry enum, and the CLI command dispatcher. That means a small public-surface change turns into a multi-file synchronization exercise by default.

How to solve it:

Make `remote-exec-proto` plus one human-facing document the canonical contract source. Reduce `AGENTS.md` and the operator skill to role-specific guidance that links back to that canonical surface. In the broker, replace manual parallel registries with one declarative tool catalog that drives MCP registration, direct mode dispatch, CLI exposure, and tool metadata.

### 2. `exec_command` contains an ad hoc `apply_patch` interception layer

Severity: High

Evidence: `crates/remote-exec-broker/src/tools/exec.rs:45`, `crates/remote-exec-broker/src/tools/exec_intercept.rs:64`, `crates/remote-exec-broker/src/tools/exec.rs:219`, `crates/remote-exec-broker/src/tools/exec_format.rs:19`, `crates/remote-exec-broker/src/tools/patch.rs:40`

Why this is a smell:

The broker currently sniffs shell command text, reroutes patch-like invocations, and fabricates exec-shaped results. That is a layering break: behavior depends on shell text and quoting heuristics instead of schema. It also creates two subtly different patch surfaces, one explicit and one hidden behind command interception.

How to solve it:

Deprecate the interception path or move it behind an explicit compatibility flag. Make `apply_patch` the only real patch surface, with one structured result shape and one validation path. If legacy command compatibility must remain, isolate it in a small adapter module with explicit tests and a planned removal path.

### 3. Rust error modeling loses type information too early

Severity: High

Evidence: `crates/remote-exec-proto/src/rpc/error.rs:9`, `crates/remote-exec-host/src/error.rs:6`, `crates/remote-exec-daemon/src/rpc_error.rs:6`, `crates/remote-exec-host/src/patch/mod.rs:50`, `crates/remote-exec-host/src/transfer/archive/mod.rs:62`, `crates/remote-exec-host/src/transfer/mod.rs:21`

Why this is a smell:

Important domain failures begin as typed concepts, but many are flattened into `String` codes or `anyhow::Error` internally and then rebuilt later with downcasts or text matching. That pattern weakens exhaustiveness and increases the chance that new error cases land in the wrong bucket or become test-coupled to wording.

How to solve it:

Keep domain errors typed until the outermost transport boundary. Introduce focused enums such as `PatchError`, `TransferArchiveError`, and a typed `HostRpcError` that carries `RpcErrorCode` directly. Perform HTTP or tool-level serialization once, at the daemon or broker edge, rather than piecemeal in each subsystem.

### 4. The C++ daemon HTTP contract is split across ad hoc layers

Severity: High

Evidence: `crates/remote-exec-daemon-cpp/src/server_routes.cpp:8`, `crates/remote-exec-daemon-cpp/src/http_connection.cpp:132`, `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:69`, `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp:11`, `crates/remote-exec-daemon-cpp/src/server_route_common.cpp:28`

Why this is a smell:

Buffered routes, streaming transfer routes, upgrade routes, and capability/version constants are handled in different places with hand-maintained checks. Any endpoint or header change requires synchronized edits across routing, transport, and codec code, which is a direct contract-drift risk relative to the Rust daemon and broker.

How to solve it:

Introduce one internal route registry for the C++ daemon that declares path, method, request mode, and response mode. Move route names, header names, and protocol version constants into one internal contract header used by all transport paths. That does not require a large abstraction layer; it requires one place of truth.

### 5. The C++ port-forward runtime has weak ownership and shutdown semantics

Severity: High

Evidence: `crates/remote-exec-daemon-cpp/src/port_tunnel_spawn.cpp:43`, `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp:4`, `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp:5`, `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp:163`

Why this is a smell:

The port-forward subsystem relies on detached or immediately untracked worker threads, manual atomic budget counters, and shared/weak pointer coordination. That is the most concurrency-heavy part of the C++ daemon, yet it has the loosest lifetime ownership model. The current shape makes reconnect, shutdown, and failure attribution harder to reason about.

How to solve it:

Move port-forward work under a joinable worker group or bounded pool owned by one service object. Give that owner explicit responsibility for cancellation, shutdown, budget enforcement, and error surfacing. This is a structural simplification, not a feature rewrite.

### 6. `remote-exec-pki` is carrying repository-specific bootstrap UX and config policy

Severity: High

Evidence: `crates/remote-exec-pki/src/lib.rs:6`, `crates/remote-exec-pki/src/manifest.rs:88`, `crates/remote-exec-admin/src/certs.rs:27`

Why this is a smell:

The PKI crate is described as reusable certificate/manifest logic, but it also renders broker and daemon config snippets with repo-specific assumptions like `[targets.<name>]`, `expected_daemon_name`, `listen`, and `/srv/work`. That blurs the boundary between reusable cryptographic primitives and operator UX for this specific repository.

How to solve it:

Keep `remote-exec-pki` focused on certificate generation, validation, secure writing, and structured manifest output. Move human-facing snippet rendering and bootstrap text into `remote-exec-admin` or a dedicated bootstrap module. That preserves reuse and makes ownership easier to review.

### 7. Daemon configuration is mirroring host runtime configuration instead of composing it

Severity: Medium

Evidence: `crates/remote-exec-daemon/src/config/mod.rs:28`, `crates/remote-exec-daemon/src/config/mod.rs:137`, `crates/remote-exec-daemon/src/config/mod.rs:179`, `crates/remote-exec-host/src/config/mod.rs:35`, `crates/remote-exec-host/src/config/mod.rs:179`, `crates/remote-exec-host/src/state.rs:61`

Why this is a smell:

The same knobs exist in `DaemonConfig`, `HostRuntimeConfig`, and `EmbeddedHostConfig`, then get copied through `From` impls before runtime state is built. That adds ceremony and raises the chance of missing one field when a new host setting is introduced.

How to solve it:

Define one validated host-settings struct in `remote-exec-host`, then make daemon config and broker-local config compose it instead of re-declaring overlapping fields. The daemon should own daemon-only concerns like transport/TLS/listen address, not re-model host runtime structure field by field.

### 8. Transfer modeling is fragmented and includes legacy shape debt

Severity: Medium

Evidence: `crates/remote-exec-proto/src/public.rs:101`, `crates/remote-exec-broker/src/tools/transfer.rs:93`, `crates/remote-exec-broker/src/bin/remote_exec.rs:608`, `crates/remote-exec-proto/src/rpc/transfer.rs:18`, `crates/remote-exec-proto/src/transfer.rs:159`, `crates/remote-exec-daemon/src/transfer/codec.rs:33`

Why this is a smell:

`transfer_files` still has mutually exclusive `source` and `sources`, and transfer metadata is represented again as request structs, metadata structs, and daemon HTTP headers. The shape is semantically one concept, but it is carried differently across broker/public/rpc/HTTP layers. That is exactly the kind of contract shape that drifts because the invariant lives in prose and helper code rather than in the type system.

How to solve it:

Standardize on `sources` with minimum length `1` as the forward shape. Keep `source` only as a deprecated broker-side alias for compatibility. For daemon transport, define one canonical transfer envelope/metadata type and keep HTTP header encoding in a thin adapter layer with its own unit tests.

### 9. Capability and identity data are duplicated in slightly different schemas

Severity: Medium

Evidence: `crates/remote-exec-proto/src/public.rs:61`, `crates/remote-exec-proto/src/rpc/target.rs:14`, `crates/remote-exec-host/src/state.rs:101`

Why this is a smell:

`ListTargetDaemonInfo` and `TargetInfoResponse` are very close but not the same. The RPC path already exposes capabilities that the broker-facing public type does not. That creates a subtle drift risk for broker caching, future capability rollout, and test coverage.

How to solve it:

Extract shared `DaemonIdentity` and `TargetCapabilities` structs in `remote-exec-proto`, then embed or flatten them into both public and RPC surfaces. That turns future capability additions into one schema change instead of two manually synchronized edits.

### 10. Test support and test seams have weak structural boundaries

Severity: Medium

Evidence: `crates/remote-exec-broker/tests/support/mod.rs:8`, `crates/remote-exec-daemon/tests/support/mod.rs:5`, `tests/support/test_helpers.rs:1`, `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs:22`, `crates/remote-exec-broker/tests/support/stub_daemon.rs:1`

Why this is a smell:

Shared test helpers are pulled in via `#[path = "..."]` file inclusion, which hides ownership boundaries and makes refactors noisy. On top of that, some scenario files have grown very large, especially the port-forward integration seams and broker stub daemon support. This usually means the production code lacks smaller verification seams, so broad scenario tests are doing too much work.

How to solve it:

Promote shared test support into a small dev-only workspace crate. Split large scenario suites by concern, such as codec, session store, reconnect semantics, TCP, and UDP. Keep a smaller number of end-to-end tunnel scenarios, but move most logic checks into narrower harnesses and reusable helpers.

### 11. Examples, tests, and build entry points already show contract drift

Severity: Medium

Evidence: `crates/remote-exec-broker/src/config.rs:34`, `configs/broker.example.toml:72`, `crates/remote-exec-broker/tests/mcp_cli.rs:24`, `crates/remote-exec-daemon-cpp/README.md:13`, `crates/remote-exec-daemon-cpp/NMakefile:72`

Why this is a smell:

The broker code defaults to structured content enabled, but the checked-in example disables it. The C++ README says the daemon builds as C++11 across supported toolchains, while the MSVC path is pinned to `/std:c++14`. These are not catastrophic bugs, but they show that documentation and build plumbing can already drift away from the effective contract.

How to solve it:

Add smoke tests that load the real example configs and verify the intended default behavior. Make the C++ build matrix data-driven from one inventory and enforce the declared language level in one canonical place used by all entry points. Separate “recommended example” from “alternate compatibility example” when defaults intentionally differ.

## Recommended Repair Order

### Phase 1: Collapse manual contract duplication

Start by creating one tool catalog and one canonical contract source. This has the highest leverage because it reduces review burden everywhere else. It also aligns with the repo’s own change-guidance sections, which currently reveal how broad the blast radius already is.

### Phase 2: Restore typed boundaries

Next, fix error and capability modeling. Keep `RpcErrorCode` typed, add typed patch/transfer errors, and extract shared capability structs. This is a contained refactor with good payoff for test quality and future feature work.

### Phase 3: Remove legacy shape debt

Simplify `transfer_files` to one source shape and promote under-typed fields like `view_image.detail` to enums. These are public-surface cleanups that will reduce normalization code and backend-specific validation drift.

### Phase 4: Rebuild the C++ internal contract seams

Introduce a C++ route registry, centralize contract constants, and move port-forward execution under supervised ownership. This is the riskiest technical area, so it should happen after the shared contract work above narrows the moving parts.

### Phase 5: Clean up test and build ownership

Finish by creating a real test-support crate, splitting the largest scenario suites, and making build/example defaults testable. This will not fix architecture by itself, but it will lower the cost of maintaining the earlier changes.

## Closing Assessment

The core issue is not “bad code” in the superficial sense. The deeper problem is that the repository is paying a tax for supporting one public contract across multiple runtimes, transports, and docs without enough mechanically enforced single sources of truth. The best improvements are the ones that remove places where humans must remember to update several layers together.
