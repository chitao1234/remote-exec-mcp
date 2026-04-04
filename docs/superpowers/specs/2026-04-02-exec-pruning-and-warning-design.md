# Exec Pruning And Warning Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-daemon/src/exec/store.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `tests/e2e/multi_target.rs`

## Goal

Close the next remaining `exec_command` and `write_stdin` compatibility gaps by implementing:

- strict per-target session pruning semantics
- model-visible warnings through `CallToolResult.meta`

The resulting behavior should stay remote-first and broker-mediated, while moving closer to the Codex-inspired semantics documented in `docs/local-system-tools.md`.

## Scope

This design covers only the following behavior changes:

- a hard session cap of `64` per target machine
- protection of the `8` most recently used sessions per target machine
- threshold-crossing warnings at `60` open sessions per target machine
- intercepted `apply_patch` warnings through MCP result metadata

This design does not cover:

- event-model parity
- pause-aware waiting
- approval or sandbox behavior
- login tool-schema gating
- any new user-visible warning text inside normal command output

## Current Behavior Summary

Today, each daemon enforces a hard limit of `64` sessions and refreshes `last_touched_at` on successful lock, but it still uses a simpler pruning policy:

- exited sessions are preferred as pruning victims
- otherwise the oldest session is pruned
- there is no protected recent set
- there is no warning threshold behavior

The broker also intercepts `apply_patch` inside `exec_command`, but it does not record the model warning described in `docs/local-system-tools.md`.

## Decision Summary

### 1. Keep pruning daemon-local and therefore per target machine

The daemon session store remains the source of truth for pruning decisions.

Because each daemon serves exactly one configured target machine, all pruning semantics remain per target machine:

- the `64` session cap is per target machine
- the protected recent set of `8` is per target machine
- the `60`-session warning threshold is per target machine

No broker-global pooling or cross-target ranking is introduced.

### 2. Adopt strict protected-recent pruning semantics

The daemon session store should track `last_touched_at` metadata for each session record.

`last_touched_at` is refreshed when:

- a session is inserted
- a session is successfully locked for `write_stdin`
- a session is otherwise reused through the store API

When inserting a new session into a full store:

1. rank all sessions by `last_touched_at`
2. protect the `8` most recently touched sessions
3. among the remaining non-protected sessions, prune the oldest exited session first
4. if none are exited, prune the oldest live session
5. terminate the pruned live session before completing the insert
6. insert the new session

For low-limit unit tests, the protected set should be capped at `limit - 1` so insertion can still succeed.

This intentionally replaces the repo-local simplified pruning policy from the earlier runtime-compat design.

### 3. Emit session-pressure warnings only on threshold crossing

The daemon should detect when its open-session count crosses from below `60` to `60` or above during `exec_command` session insertion.

The warning is emitted only on that crossing event.

That means:

- no warning at `0..59`
- one warning on `59 -> 60`
- no repeated warning on `60 -> 61`, `61 -> 62`, and so on
- if the count later drops below `60` and then rises to `60` again, the warning is emitted again on that new crossing

The warning is attached to the `exec_command` result that created the threshold crossing.

`write_stdin` does not emit this warning.

### 4. Use one shared warning surface in `CallToolResult.meta`

Warnings should not be appended to the normal text output.

Instead, warnings should be carried through MCP result metadata using a stable shape:

```json
{
  "warnings": [
    {
      "code": "apply_patch_via_exec_command",
      "message": "Use apply_patch directly rather than through exec_command."
    }
  ]
}
```

and:

```json
{
  "warnings": [
    {
      "code": "exec_session_limit_approaching",
      "message": "Target `builder-a` now has 60 open exec sessions."
    }
  ]
}
```

If more than one warning applies to the same result, they should be combined into the same `warnings` array.

### 5. Emit intercepted `apply_patch` warnings for every intercepted attempt

The intercepted `apply_patch` warning should be emitted for every intercepted `exec_command` attempt:

- intercepted patch apply success
- intercepted patch apply failure

This warning is attached even when the intercepted patch fails and the broker returns a tool error.

Non-intercepted commands do not receive this warning.

## Data Flow

### Exec start

1. broker forwards `exec_command` to the target daemon
2. daemon determines whether the new running session will be inserted into the store
3. daemon store prunes old sessions as needed using the protected-recent policy
4. daemon store computes whether the insert crossed the `60`-session threshold
5. daemon returns normal exec response plus optional warning payload
6. broker maps daemon warning payload into `CallToolResult.meta`

### Intercepted apply_patch

1. broker recognizes an intercepted `apply_patch` shell form
2. broker routes it through the patch pathway instead of exec
3. broker adds the `apply_patch_via_exec_command` warning to the MCP result metadata
4. broker returns either:
   - a successful tool result with warning metadata
   - or a tool-error result with the same warning metadata

## Code Boundaries

### `crates/remote-exec-daemon/src/exec/store.rs`

- extend pruning logic to protect the `8` most recently touched sessions
- keep exited-session preference within the non-protected set
- detect threshold crossing from below `60` to `60` or above
- return structured insert outcome rather than only mutating internal state

### `crates/remote-exec-daemon/src/exec/mod.rs`

- consume the store insert outcome
- include optional warning data in the exec RPC response only for `exec_command`

### `crates/remote-exec-proto/src/rpc.rs`

- add optional warning payload support to the exec response shape shared between daemon and broker

### `crates/remote-exec-broker/src/tools/exec.rs`

- map daemon exec warnings into broker MCP result metadata
- add intercepted `apply_patch` warning metadata on both success and error paths

### `crates/remote-exec-broker/src/mcp_server.rs`

- allow both successful and error tool results to carry optional metadata
- keep existing text output unchanged

## Error Handling

- If pruning must evict a live session, the daemon should terminate it best-effort and then remove it from the store.
- If a later `write_stdin` call reaches a session that was pruned, the daemon continues to report `unknown_session`, and the broker continues to surface `Unknown process id <session_id>`.
- Warning metadata is additive and should never change whether a tool call is considered success or error.
- If the daemon does not attach a warning, the broker should omit warning metadata entirely instead of sending an empty `warnings` array.

## Rejected Alternatives

### Keep the earlier simplified repo-local pruning policy

This would retain:

- a hard cap of `64`
- plain LRU pruning with exited-session preference
- no protected recent set
- no threshold warnings

It is rejected because the next requested target is explicit parity for pruning semantics, not just bounded storage.

### Emit warnings in normal tool text

This would append warnings to:

- intercepted `apply_patch` text output
- `exec_command` text output when session counts cross `60`

It is rejected because it pollutes the normal user-facing command output and mixes advisory data with command results.

### Emit the `60`-session warning on every call while above threshold

This would be simpler than tracking threshold crossing state.

It is rejected because it would spam the model and make the warning less useful.

## Testing Plan

Add focused daemon store tests for:

- protecting the newest `8` sessions from pruning
- pruning the oldest exited non-protected session before any live non-protected session
- pruning the oldest live non-protected session when all non-protected sessions are still running
- emitting the threshold-crossing warning on `59 -> 60`
- not re-emitting the warning while remaining at or above `60`
- re-emitting the warning after dropping below `60` and crossing again

Add broker tests for:

- intercepted `apply_patch` success includes warning metadata
- intercepted `apply_patch` failure still includes warning metadata
- forwarded `exec_command` includes warning metadata when the daemon reports threshold crossing
- normal non-intercepted exec results omit warning metadata when no warning applies

Finish with the existing workspace quality gate:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Success Criteria

This design is complete when:

- each daemon keeps at most `64` open sessions
- the `8` most recently touched sessions on a daemon are protected from pruning
- exited non-protected sessions are still preferred as victims
- the `60`-session warning appears only when a daemon crosses the threshold
- intercepted `apply_patch` attempts always include warning metadata
- warning metadata never alters the normal command-output text surface
