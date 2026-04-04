# List Targets Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `README.md`
- `crates/remote-exec-broker/src/config.rs`
- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-broker/src/lib.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/src/tools/targets.rs`
- `crates/remote-exec-broker/tests/mcp_assets.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`

## Goal

Expand `list_targets` from names-only broker discovery into a richer broker-local discovery tool that returns:

- the configured logical target name
- a nullable cached subset of daemon metadata when the broker currently considers that metadata usable

The tool should stay broker-local and cheap to call. It should not perform live daemon refreshes during `list_targets` itself.

## Scope

Included:

- a breaking structured-result change for `list_targets`
- cached daemon metadata in `list_targets` results
- broker-side cache population, refresh, and invalidation rules
- richer model-facing text output when daemon metadata is available
- broker-facing tests for both the public output shape and cache lifecycle
- README and spec updates

Excluded:

- live daemon probing during `list_targets`
- `daemon_instance_id` exposure
- `supports_image_read` exposure
- daemon RPC changes
- new target-status enums or explicit freshness-state fields
- pagination or filtering

## Current Behavior Summary

Today `list_targets` is a broker-only read-only tool that returns:

- structured content as `targets: ["builder-a", "builder-b"]`
- text output as a simple bullet list of names

It does not expose any daemon metadata.

The broker already talks to daemons through `target_info` during startup and later verification paths, but that data is not currently cached in a form that `list_targets` returns.

## Decision Summary

### 1. Keep `list_targets` broker-local

`list_targets` remains a broker-local read-only tool.

Calling it must not trigger live daemon network requests. The tool only reads broker-held configuration and broker-held cached daemon metadata.

This preserves the current advantages of the tool:

- low latency
- no network dependency at read time
- no new failure mode tied to daemon reachability

### 2. Make the structured result a breaking object list

The structured result changes from `targets: Vec<String>` to `targets: Vec<ListTargetEntry>`.

Each entry has:

- `name: String`
- `daemon_info: ListTargetDaemonInfo | null`

Proposed result shape:

```json
{
  "targets": [
    {
      "name": "builder-a",
      "daemon_info": {
        "daemon_version": "0.1.0",
        "hostname": "builder-a",
        "platform": "linux",
        "arch": "x86_64",
        "supports_pty": true
      }
    },
    {
      "name": "builder-b",
      "daemon_info": null
    }
  ]
}
```

This is intentionally a breaking change because the tool is still young and the object-list shape is cleaner than trying to preserve `Vec<String>` while adding a second parallel metadata field.

### 3. Expose only a reduced daemon-info subset

When cached daemon metadata is usable, `daemon_info` should include only:

- `daemon_version`
- `hostname`
- `platform`
- `arch`
- `supports_pty`

The tool should not expose:

- `daemon_instance_id`
- `supports_image_read`
- broker config fields such as `base_url`
- certificate or trust configuration

This keeps the public API focused on human-useful target context without leaking restart identity or configuration details that were not requested.

### 4. Use `null` when cached info is not currently usable

`daemon_info` is `null` when:

- the broker has never verified the target successfully
- startup transport to the target failed
- cached info was cleared after later evidence that the target is unavailable
- cached info was cleared after later evidence that the daemon identity effectively changed

The absence should be explicit through `daemon_info: null`, not by omitting the field.

This keeps the schema stable and makes the difference between "known target, unknown current daemon info" and "field missing due to version drift" unambiguous.

### 5. Only expose cached info while the broker still considers it usable

Cached daemon info is visible only while the broker still considers it usable.

That means:

- successful startup verification populates the cache
- successful later verification refreshes the cache
- transport evidence that a target is unavailable clears the cache
- existing daemon-instance mismatch evidence clears the cache

The broker should therefore treat cached daemon info as tied to its current trust in the daemon relationship, not as historical telemetry.

This avoids returning stale daemon metadata after the broker learns that the target is currently unavailable or no longer matches the previous daemon instance.

### 6. Enrich the model-facing text output too

The plain text output should include a compact per-target summary when `daemon_info` exists.

Proposed text rendering:

```text
Configured targets:
- builder-a: linux/x86_64, host=builder-a, version=0.1.0, pty=yes
- builder-b
```

The text output should still be:

```text
No configured targets.
```

for the empty case.

This uses only the same fields already present in `name + daemon_info`. It does not add any extra public data beyond the structured result.

### 7. Keep ordering explicit and stable

Ordering remains lexicographic ascending by target name.

This is still part of the contract and should continue to match the broker's `BTreeMap` storage.

## Cache Lifecycle

### Startup

During broker startup:

- if initial `target_info` succeeds and identity matches, cache the reduced daemon-info subset
- if startup transport fails, leave the target configured but store no usable daemon info
- if startup returns a non-transport RPC or decode error, keep the current behavior and fail broker startup

Targets that are unavailable at startup should therefore still appear in `list_targets`, but with `daemon_info: null`.

### Successful later verification

When a later call goes through a verification path and `target_info` succeeds:

- refresh the cached daemon-info subset from that successful response
- keep the target usable for forwarded calls

This is how a target that was initially unavailable can later begin returning populated `daemon_info` in `list_targets`.

### Later unavailability evidence

If the broker later gets transport-level evidence that a target is unavailable, it should clear cached daemon info for that target.

This invalidation should happen on:

- transport failure in verification paths
- transport failure from forwarded daemon calls where reachability is directly disproven

Regular daemon RPC application errors should not clear cached daemon info, because those errors still show that the daemon is reachable and speaking the protocol.

### Later daemon-identity mismatch evidence

If the broker hits the existing daemon-instance mismatch evidence in exec session handling, it should clear cached daemon info for that target.

This keeps `list_targets` from returning the old daemon metadata after the broker has already learned that the daemon relationship changed enough to invalidate existing session assumptions.

### `list_targets` itself

`list_targets` does not refresh, probe, or mutate the cache.

It only reads broker state and renders what is currently considered usable.

## Data Flow

### `list_targets`

1. MCP client calls `list_targets` with an empty object.
2. Broker reads configured target names from `state.targets`.
3. Broker reads the cached daemon-info subset attached to each target handle.
4. Broker converts each target into:
   - `name`
   - `daemon_info` object or `null`
5. Broker formats both:
   - structured JSON output
   - compact human-readable text output
6. Broker returns the result without contacting any daemon.

### Cache population and refresh

1. broker startup or later verification obtains `TargetInfoResponse`
2. broker verifies expected target identity if configured
3. broker reduces the response to the public cached subset
4. broker stores the reduced subset on the target handle

### Cache clearing

1. broker sees transport failure or daemon-instance mismatch evidence
2. broker clears the cached daemon-info subset for that target
3. subsequent `list_targets` returns `daemon_info: null` for that target until a later successful verification repopulates it

## Code Boundaries

### `crates/remote-exec-proto/src/public.rs`

- replace the names-only `ListTargetsResult` entry shape
- add:
  - `ListTargetEntry`
  - `ListTargetDaemonInfo`
- keep `ListTargetsInput` as an empty object

### `crates/remote-exec-broker/src/lib.rs`

- extend `TargetHandle` with cached daemon-info storage
- add small helper paths to:
  - write cached daemon info from a successful `TargetInfoResponse`
  - clear cached daemon info
  - read a snapshot for `list_targets`
- update startup construction to populate or leave cache empty
- update `ensure_identity_verified` to refresh the cache on success and clear it on transport failure before returning the error

### `crates/remote-exec-broker/src/tools/targets.rs`

- build the new structured result entries
- render the compact summary text
- keep the empty-state text behavior

### `crates/remote-exec-broker/src/tools/exec.rs`

- clear cached daemon info on the existing daemon-instance mismatch path
- clear cached daemon info on forwarded transport-failure paths that prove the target is currently unavailable

### `crates/remote-exec-broker/src/tools/image.rs`

- keep the public tool contract unchanged
- clear cached daemon info on transport failure after a daemon call attempt

### `crates/remote-exec-broker/src/tools/patch.rs`

- keep the public tool contract unchanged
- clear cached daemon info on transport failure after a daemon call attempt

### `crates/remote-exec-broker/tests/mcp_assets.rs`

- update `list_targets` assertions for the new breaking object-list result
- verify richer text rendering
- verify a configured-but-unavailable target still appears with `daemon_info: null`

### `crates/remote-exec-broker/tests/mcp_exec.rs` or focused broker unit tests

- verify cache clearing when daemon mismatch or transport evidence invalidates previously cached info

### `README.md`

- update `list_targets` documentation to say it returns names plus cached daemon metadata when available

## Error Handling

`list_targets` still should not fail just because a target is unavailable.

Expected behavior:

- configured targets are always listed
- unavailable targets render with `daemon_info: null`
- no freshness-status field is returned
- no warnings metadata is needed

Transport errors matter only as cache invalidation signals for future `list_targets` output and as normal failures for the forwarding call that observed them.

## Testing

Add or update broker-facing tests for:

- successful `list_targets` with one populated cached entry
- `daemon_info: null` for a configured target that is unavailable at startup
- stable lexicographic ordering
- compact summary text rendering
- empty configured target set
- read-only annotation exposure

Add focused broker tests for cache lifecycle:

- startup success populates cache
- startup transport failure leaves cache empty
- later successful verification repopulates cache
- transport failure clears cache
- daemon-instance mismatch clears cache

No daemon tests are required because the new behavior is broker-local and uses existing daemon RPCs.

## Rejected Alternatives

### Refresh daemon info on every `list_targets` call

This would make `list_targets` return fresher data, but it would also:

- add latency
- add network dependence to discovery
- introduce new runtime failure modes for a tool that should remain cheap and reliable

It is rejected because the requested direction is cached daemon data, not a live polling tool.

### Keep `targets: Vec<String>` and add a parallel metadata map

This would avoid a breaking change.

It is rejected because the object-list result is much cleaner and the user explicitly allowed a breaking change.

### Expose a freshness or verification-state field

This would add something like:

- `daemon_info_state`
- `verified`
- `unknown`

It is rejected because the chosen contract is simpler: expose the reduced metadata when usable, otherwise return `daemon_info: null`.

### Expose `daemon_instance_id`

This would give callers a stronger restart-identity signal.

It is rejected because the requested public payload is a reduced subset, and `daemon_instance_id` is more operationally sensitive and more likely to create downstream coupling than the selected human-readable metadata.

### Expose `supports_image_read`

This would mirror more of the daemon's `target_info` response.

It is rejected because the requested reduced subset explicitly excludes it.

## Verification

Targeted checks after implementation:

- `cargo test -p remote-exec-broker --test mcp_assets`
- `cargo test -p remote-exec-broker --test mcp_exec`
- `cargo test -p remote-exec-broker --lib`

Broader checks for the final implementation batch:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
