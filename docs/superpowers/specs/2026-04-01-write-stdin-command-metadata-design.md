# Write Stdin Command Metadata Design

Status: approved design captured in writing

Date: 2026-04-01

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-broker/src/session_store.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `crates/remote-exec-proto/src/public.rs`

## Goal

Preserve the original `exec_command` command string across later `write_stdin` calls so broker responses can continue to identify the session with the original command metadata.

This should close the currently implemented gap where `write_stdin` keeps the public session alive but loses the original command text that started that session.

## Scope

This design covers only broker-side preservation and reuse of original command metadata for interactive exec sessions.

Included:

- storing the original command string in the broker session store when `exec_command` creates a live public session
- reusing that stored command when formatting `write_stdin` text output
- exposing the stored command in the structured tool result for both `exec_command` and `write_stdin`
- adding broker tests for text and structured output preservation

Excluded:

- pause-aware waiting
- daemon-side session metadata changes
- daemon RPC request or response changes
- event model additions such as `TerminalInteractionEvent`
- any changes to session routing, invalidation, or timeout rules

## Current Behavior Summary

Today the broker stores only routing metadata for a live session:

- public `session_id`
- owning `target`
- daemon-local `daemon_session_id`
- daemon instance identity

That is enough to forward later `write_stdin` calls to the correct daemon, but not enough to reconstruct the original session command. As a result:

- the `write_stdin` text response omits the leading `Command: ...` line
- the structured result does not carry session command metadata

This diverges from the behavior documented in `docs/local-system-tools.md`, which says later `write_stdin` responses preserve original session command metadata.

## Decision Summary

### 1. Store original command metadata in the broker session record

When `exec_command` returns a running session, the broker should store the original `input.cmd` alongside the existing routing fields in the session record.

This metadata belongs in the broker because:

- the broker owns the public session namespace
- the broker already has the canonical user-facing command string
- later `write_stdin` calls are resolved through the broker session store

No daemon changes are required.

### 2. Restore the original command in `write_stdin` text output

`format_poll_text(...)` should accept the stored session command and include the same `Command: ...` line format used by `exec_command` text output whenever the original command is known.

This means a later poll should continue to identify the session by the command that created it instead of only showing chunk, wall time, and process status.

If the original command is unavailable for any reason, formatting should continue to work without it rather than failing.

### 3. Expose optional `session_command` in structured results

Extend the public structured result type with:

- `session_command?: string`

Population rules:

- `exec_command` should include `session_command` with the original command string
- `write_stdin` should include `session_command` from the broker session record
- non-exec tools remain unchanged

This keeps the structured output aligned with the restored text metadata and makes the preserved command available to callers that do not parse the text block.

## Rejected Alternatives

### Daemon-roundtrip metadata

This would add command metadata to daemon session state and have `/v1/exec/write` echo it back.

It was rejected because the broker already owns the public session and already has the exact command string we need. Extending the daemon RPC surface would add complexity without changing the correctness of the result.

### Text-only restoration

This would restore `Command: ...` in broker text output but leave structured output unchanged.

It was rejected because it preserves only one output surface and leaves the structured result incomplete for tool consumers that depend on metadata fields rather than formatted text.

## Code Boundaries

### `crates/remote-exec-broker/src/session_store.rs`

- extend `SessionRecord` with the original command string
- require the command when inserting a new running session

### `crates/remote-exec-broker/src/tools/exec.rs`

- store `input.cmd` in the session record created by `exec_command`
- use stored session metadata when building `write_stdin` text and structured outputs

### `crates/remote-exec-broker/src/mcp_server.rs`

- update poll-text formatting so the original session command is rendered when known

### `crates/remote-exec-proto/src/public.rs`

- add optional `session_command` to `CommandToolResult`

### `crates/remote-exec-broker/tests/mcp_exec.rs`

- add coverage proving `write_stdin` text output includes the original command
- add coverage proving `write_stdin` structured output includes `session_command`

## Data Flow

### Exec start

1. user calls broker `exec_command`
2. broker forwards start request to the daemon
3. if the daemon reports a still-running session, broker allocates a public `session_id`
4. broker stores routing metadata plus the original command string in its session store
5. broker returns text and structured output including that command metadata

### Write stdin / poll

1. user calls broker `write_stdin`
2. broker resolves the public `session_id` to a stored broker session record
3. broker forwards the write or poll request to the owning daemon
4. broker formats the response using the stored original command string
5. broker returns structured output including `session_command`
6. if the daemon reports process exit, broker removes the session as it already does today

## Error Handling

- Unknown-session behavior is unchanged.
- Target-mismatch validation is unchanged.
- Retryable daemon errors continue to keep the broker session record intact.
- If a stored session record somehow lacks command metadata, `write_stdin` should still succeed and simply omit the `Command: ...` line and `session_command` value.

## Testing Plan

Add focused broker tests for:

- `write_stdin` text output includes `Command: <original cmd>`
- `write_stdin` structured output includes `session_command`
- `exec_command` structured output includes `session_command`

Existing tests already cover routing and retryable-session retention, so this change only needs to extend broker exec coverage rather than add daemon tests.

## Success Criteria

This design is complete when:

- the broker session store preserves the original `exec_command` string for live sessions
- later `write_stdin` responses include `Command: ...` when the original command is known
- `CommandToolResult` exposes optional `session_command`
- `exec_command` and `write_stdin` both populate `session_command`
- existing session routing and invalidation behavior remains unchanged
