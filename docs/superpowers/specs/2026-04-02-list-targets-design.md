# List Targets Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `README.md`
- `crates/remote-exec-broker/src/config.rs`
- `crates/remote-exec-broker/src/lib.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/tests/mcp_assets.rs`
- `crates/remote-exec-proto/src/public.rs`

## Goal

Add a broker-local `list_targets` MCP tool so agents can discover the configured logical target names without relying on broker config files or out-of-band knowledge.

The first version should stay intentionally narrow:

- config-only discovery
- target names only
- stable ordering
- no daemon probing

## Scope

Included:

- a new public broker MCP tool named `list_targets`
- no-input invocation shape
- text output plus structured JSON output
- lexicographic ascending ordering as part of the contract
- read-only advertisement in tool annotations
- broker-facing tests and README updates

Excluded:

- daemon reachability checks
- per-target health or capability metadata
- daemon instance details such as hostname, platform, or arch
- dynamic filtering or pagination
- daemon RPC changes

## Current Behavior Summary

Today the broker requires callers to already know target names.

The broker does have the configured target set in memory as `BrokerState.targets`, but there is no public MCP tool that exposes that list. The only current public tools are:

- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

This means target discovery is currently implicit rather than part of the public tool contract.

## Decision Summary

### 1. Add a broker-only `list_targets` tool

`list_targets` should be implemented entirely in the broker.

It reads the configured logical target names from broker state and returns them without contacting any daemon. This keeps the call cheap, deterministic, and usable even when some targets are unavailable.

### 2. Return names only

The first version should return only logical target names.

It should not expose:

- `base_url`
- certificate paths
- `expected_daemon_name`
- live daemon-reported information

This keeps the public surface minimal and avoids committing to metadata fields that may need future policy discussion.

### 3. Make ordering explicit and stable

Ordering should be a defined part of the API.

Results are returned in lexicographic ascending order. This matches the broker's existing `BTreeMap` storage and gives callers stable, predictable output.

### 4. Use a zero-information input shape

The tool should accept no meaningful arguments.

The public input type should therefore be an empty object shape. The intended invocation is:

```json
{}
```

No `target` argument is required because this tool is not machine-local. It is broker-local discovery.

### 5. Return both text and structured output

The tool should follow the existing broker pattern of returning:

- model-facing text content
- structured JSON content

Structured result:

```json
{
  "targets": ["builder-a", "builder-b"]
}
```

Text result for non-empty output:

```text
Configured targets:
- builder-a
- builder-b
```

Text result for empty output:

```text
No configured targets.
```

The empty case should still return structured content:

```json
{
  "targets": []
}
```

### 6. Advertise the tool as read-only

`list_targets` should be marked with `read_only_hint = true`.

The tool only reads broker-held configuration state and has no side effects.

## Data Flow

1. MCP client calls `list_targets` with an empty object.
2. Broker handler reads `state.targets.keys()`.
3. Broker converts the keys into a lexicographically ordered `Vec<String>`.
4. Broker formats:
   - text content for the conversation surface
   - structured content for machine consumption
5. Broker returns the result with `read_only_hint = true` on the declared tool.

No daemon RPC is issued at any step.

## Code Boundaries

### `crates/remote-exec-proto/src/public.rs`

- add an empty `ListTargetsInput`
- add a `ListTargetsResult` with `targets: Vec<String>`

### `crates/remote-exec-broker/src/mcp_server.rs`

- register the new `list_targets` tool
- mark it read-only
- route it to a broker-local handler

### `crates/remote-exec-broker/src/tools/`

- add a broker-local handler that:
  - reads target names from `BrokerState`
  - formats model-facing text
  - returns structured JSON

### `README.md`

- add `list_targets` to the supported tools list
- document that target discovery is broker-local and config-based

## Error Handling

`list_targets` should have no daemon-availability failure mode because it does not contact daemons.

Expected behavior:

- configured-but-unreachable targets still appear in the result
- identity verification state has no effect on listing
- an empty broker config returns success with an empty list and `No configured targets.`

No special warning metadata is needed for this tool.

## Testing

Add broker-facing tests for:

- successful listing with two configured targets
- lexicographic ordering even when config insertion order differs
- empty configured target set using a focused broker-state test
- read-only annotation exposure from tool listing

No daemon tests are required because the behavior is fully broker-local.

## Rejected Alternatives

### Probe daemons during `list_targets`

This would attach availability or identity information at call time.

It is rejected for the first version because:

- the user explicitly asked for config-only behavior
- it would make results depend on transient network state
- it would create new failure modes for a discovery tool that should stay cheap and reliable

### Return broker config metadata such as `base_url`

This would make the result richer immediately.

It is rejected because the first user need is simply discovering valid logical target names. Exposing extra broker config fields expands the public contract without clear benefit yet.

### Reuse `target_info` as the discovery mechanism

This would lean on the existing daemon RPC endpoint.

It is rejected because `target_info` is per-daemon, not a broker discovery contract, and it would couple target listing to daemon reachability.

## Verification

Targeted checks after implementation:

- `cargo test -p remote-exec-broker --test mcp_assets`
- `cargo test -p remote-exec-broker --test mcp_exec`

Broader checks for the final implementation batch:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
