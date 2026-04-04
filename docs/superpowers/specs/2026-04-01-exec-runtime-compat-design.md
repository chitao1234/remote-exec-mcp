# Exec Runtime Compatibility Design

Status: approved design captured in writing

Date: 2026-04-01

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-daemon/src/exec/mod.rs`
- `crates/remote-exec-daemon/src/exec/session.rs`
- `crates/remote-exec-daemon/src/exec/store.rs`
- `crates/remote-exec-daemon/tests/exec_rpc.rs`

## Goal

Close the highest-value `exec_command` and `write_stdin` compatibility gaps first by implementing:

- child-process environment normalization
- a daemon-side default `max_output_tokens` cap of `10000`
- bounded daemon-local session storage with LRU-style pruning

The resulting behavior should stay remote-first and daemon-local, while moving closer to the Codex-inspired semantics documented in `docs/local-system-tools.md`.

## Scope

This design covers only the daemon execution runtime for the following behavior changes:

- normalized child env for both PTY and pipe-backed processes
- default output truncation cap when callers omit `max_output_tokens`
- a hard limit of `64` sessions per target machine, enforced by each daemon's session store

This design does not cover:

- `apply_patch` interception in `exec_command`
- event model compatibility
- pause-aware waiting
- login-shell policy changes
- approval or sandbox logic
- broker-side session semantics beyond existing invalidation behavior

## Current Behavior Summary

Today, the daemon launches child processes without applying the Codex-style env overlay, only truncates output when the caller explicitly provides `max_output_tokens`, and stores sessions in an unbounded in-memory map.

That leaves three concrete issues:

- command behavior can vary based on inherited environment noise such as pagers and color settings
- output can grow without the expected default cap when callers omit `max_output_tokens`
- long-running or abandoned sessions can accumulate without a hard bound

## Decision Summary

### 1. Normalize child environment at spawn time

Every child process launched by the daemon should receive a fixed env overlay in addition to the inherited environment.

The overlay is:

- `NO_COLOR=1`
- `TERM=dumb`
- `LANG=C.UTF-8`
- `LC_CTYPE=C.UTF-8`
- `LC_ALL=C.UTF-8`
- `COLORTERM=`
- `PAGER=cat`
- `GIT_PAGER=cat`
- `GH_PAGER=cat`
- `CODEX_CI=1`

This applies to both:

- PTY-backed sessions
- pipe-backed sessions

Shell resolution remains unchanged. This feature only affects the environment seen by the launched command.

### 2. Apply a default `max_output_tokens` cap of `10000`

When `max_output_tokens` is omitted, the daemon should treat it as `10000`.

When `max_output_tokens` is explicitly provided, the caller-supplied value wins, including `0`.

This rule should be applied in the shared output snapshot path so the same effective cap is used for:

- `exec_command` initial responses
- `write_stdin` poll responses
- final responses after process exit

The existing truncation algorithm and `original_token_count` behavior stay the same.

### 3. Enforce a hard session cap per target machine

The hard cap is `64` sessions per daemon session store.

Because each daemon serves exactly one configured target machine, that means the limit is per target machine rather than global at the broker.

The store should track `last_touched_at` metadata for each session record.

`last_touched_at` is refreshed when:

- a session is inserted
- a session is successfully locked for `write_stdin`
- a session is otherwise reused through the store API

When inserting a new session into a full store:

1. prune the least-recently-used exited session first
2. if none are exited, terminate and remove the least-recently-used live session
3. insert the new session

This intentionally does not implement the more complex Codex rule that protects the newest eight sessions. The goal here is bounded memory and good behavior for actively used sessions with a simpler repo-local policy.

## Rejected Alternatives

### Strict Codex pruning policy

This would add:

- a protected recent set of eight sessions
- a warning threshold at sixty open sessions
- more complex ranking and pruning rules

It was rejected for now because it adds more moving parts without changing the core guarantee we need first: bounded daemon-local session storage with sensible eviction.

### Reject new sessions when the store is full

This would keep the cap but fail `exec_command` once all sixty-four slots are occupied.

It was rejected because it creates avoidable user-visible errors in cases where old abandoned sessions could be cleaned up automatically.

## Code Boundaries

### `crates/remote-exec-daemon/src/exec/session.rs`

- add a shared env-overlay helper
- apply the overlay in both PTY and pipe spawn paths
- add a session termination path that can be called during pruning
- expose a lightweight way for the store to ask whether a session has already exited

### `crates/remote-exec-daemon/src/exec/output.rs`

- add a shared default-cap constant of `10000`
- add a helper that resolves the effective token cap from the optional request value
- keep the existing truncation algorithm unchanged aside from using the default when omitted

### `crates/remote-exec-daemon/src/exec/store.rs`

- replace the plain session map value with a record that includes session metadata
- track `last_touched_at`
- implement pruning on insert using least-recently-used ordering
- prefer exited sessions before terminating live sessions

### `crates/remote-exec-daemon/src/exec/mod.rs`

- route response snapshot creation through the default-cap logic
- keep broker and RPC response shapes unchanged
- continue returning `unknown_session` when a pruned daemon session is later polled

## Data Flow

### Exec start

1. daemon resolves cwd and shell argv
2. daemon spawns the child with normalized env
3. daemon collects initial output
4. daemon snapshots output using the effective token cap
5. if the process is still running, daemon inserts the session into the capped store
6. store prunes older sessions if necessary before accepting the new one

### Write stdin / poll

1. daemon locks the existing session
2. store refreshes that session's `last_touched_at`
3. daemon writes stdin or performs an empty poll
4. daemon snapshots output using the effective token cap
5. if the process exited, the daemon retires the session as it already does

## Error Handling

- If pruning must evict a live session, the daemon should terminate it best-effort and then drop it from the store.
- If a later poll reaches a session that was pruned, the daemon continues to report `unknown_session`, which the broker already translates into session invalidation behavior.
- Env normalization should not add new user-visible failures in normal operation; it should use standard command env injection supported by the existing spawn backends.

## Testing Plan

Add focused daemon tests for:

- env normalization in pipe mode
- env normalization in PTY mode
- default truncation when `max_output_tokens` is omitted
- explicit truncation override still working when `max_output_tokens` is supplied

Add store-level tests for:

- `last_touched_at` refreshing on lock
- pruning the least-recently-used exited session before any live session
- pruning the least-recently-used live session when all sessions are still running

No new pruning-specific exec RPC test is required in this phase. The pruning policy should be proven at the store level, while existing daemon and broker behavior continues to cover `unknown_session` handling.

## Success Criteria

This design is complete when:

- spawned commands see the normalized env overlay in both PTY and pipe modes
- omitted `max_output_tokens` behaves as if `10000` was supplied
- each daemon keeps at most `64` session records
- session pruning prefers exited LRU sessions before live LRU sessions
- existing broker-facing session invalidation behavior remains intact
