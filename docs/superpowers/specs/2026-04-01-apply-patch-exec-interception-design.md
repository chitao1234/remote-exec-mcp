# Apply Patch Exec Interception Design

Status: approved design captured in writing

Date: 2026-04-01

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/src/tools/patch.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `crates/remote-exec-broker/tests/support/mod.rs`
- `crates/remote-exec-proto/src/public.rs`

## Goal

Bring `exec_command` closer to Codex behavior by intercepting explicit shell-style `apply_patch` invocations before daemon exec launch and routing them through the existing broker `apply_patch` path.

This batch focuses on the interception behavior only:

- recognize the documented explicit `apply_patch` and `applypatch` shell/argv forms
- skip daemon `exec_start` when interception matches
- execute the existing direct `apply_patch` broker flow instead
- return the intentionally wrapped unified-exec-style output Codex uses for intercepted patch calls

## Scope

Included:

- broker-side interception before any daemon exec RPC
- support for the documented explicit forms:
  - `apply_patch "<patch>"`
  - `applypatch "<patch>"`
  - `apply_patch <<'EOF' ... EOF`
  - `applypatch <<'EOF' ... EOF`
  - `cd <path> && apply_patch <<'EOF' ... EOF`
  - `cd <path> && applypatch <<'EOF' ... EOF`
- relative `cd <path>` handling layered on top of the existing `exec_command.workdir`
- unified-exec-shaped wrapped success output for intercepted calls
- broker tests proving interception and non-interception behavior

Excluded:

- freeform/custom-tool `apply_patch`
- patch events
- warning emission telling the model to use `apply_patch` directly
- implicit-invocation rejection for raw patch bodies
- daemon or runtime self-invocation changes
- approval and sandbox compatibility work
- any new path restriction hardening beyond current behavior
- changes to direct `apply_patch` output or verification behavior

## Compatibility Interpretation

The compatibility target for this batch is the `exec_command` interception behavior documented in `docs/local-system-tools.md`, not a general shell parser.

That means the broker should:

- conservatively recognize only the documented explicit forms
- treat recognized forms as intercepted patch work instead of process execution
- keep the intercepted output surface intentionally odd:
  - model-facing text still looks like wrapped unified exec output
  - structured content still uses unified-exec JSON fields
  - direct `apply_patch` still returns summary text plus `{}`

## Current Behavior Summary

Today `exec_command` always forwards the request to daemon `exec_start`.

That leaves these mismatches relative to the documented behavior:

1. explicit `apply_patch` shell commands are treated as ordinary process execution
2. no broker-side parser exists for accepted direct, alias, or heredoc interception forms
3. no wrapped intercepted success shape exists
4. intercepted calls cannot override the effective workdir through `cd <path> && ...`

## Decision Summary

### 1. Interception lives in the broker before `exec_start`

The broker should inspect `ExecCommandInput` at the start of `exec_command`.

If the command matches an accepted explicit patch form:

- do not call daemon `exec_start`
- do not allocate or persist a broker session
- run the existing broker patch forwarding path instead

If the command does not match:

- continue through the existing unified exec path unchanged

This matches the compatibility notes, which place interception before process launch.

### 2. Match only documented explicit forms

The parser should intentionally be narrow.

Accepted forms for this batch:

- direct invocation:
  - `apply_patch "<patch>"`
  - `applypatch "<patch>"`
- heredoc invocation:
  - `apply_patch <<'EOF' ... EOF`
  - `applypatch <<'EOF' ... EOF`
- wrapped heredoc invocation:
  - `cd <path> && apply_patch <<'EOF' ... EOF`
  - `cd <path> && applypatch <<'EOF' ... EOF`

Required non-matches:

- extra leading or trailing commands
- `;`, `||`, or pipes around the patch invocation
- `cd` with more than one positional argument
- raw patch bodies with no explicit `apply_patch` command
- any shell text outside the documented alias/direct/heredoc patterns

This keeps the batch aligned with the documented conservative wrapper parser instead of inventing a broader shell grammar.

### 3. Preserve Codex's wrapped intercepted output shape

Intercepted success should not reuse the direct `apply_patch` result shape.

Instead it should return unified-exec-style output with patch summary text placed in the `output` field and formatted through a dedicated intercepted formatter.

Expected model-facing success text:

- includes `Wall time: 0.000 seconds`
- includes `Process exited with code 0`
- includes `Output:`
- includes the patch summary text after `Output:`
- does not include `Command:`
- does not include `Chunk ID:`

Expected structured success content:

- `target`
- `chunk_id = null`
- `wall_time_seconds = 0.0`
- `exit_code = 0`
- `session_id = null`
- `session_command = null`
- `original_token_count = null`
- `output = "<patch summary>"`

This deliberately preserves the documented mismatch between direct `apply_patch` and intercepted `exec_command`.

### 4. Intercepted failures surface as tool errors

When an explicit intercepted form matches but patch parse or verification fails:

- the broker should still treat the request as intercepted
- the broker should still avoid daemon `exec_start`
- the patch path should fail as a tool error

This is not modeled as a synthetic non-zero process exit payload.

Command text that does not match the interception forms remains normal `exec_command` input and is not specially rejected in this batch.

## Rejected Alternatives

### Normalize intercepted output to direct `apply_patch`

This would return summary text plus `{}` even when the request came through `exec_command`.

It was rejected because the compatibility notes explicitly document the wrapped unified-exec output shape for intercepted patch calls.

### Teach the daemon to intercept patch commands

This would push shell parsing and rerouting into the daemon exec path.

It was rejected because the documented behavior places interception before process launch and because broker-local interception is cleaner with the current architecture.

### Add implicit raw-patch rejection in this batch

This would reject command text that looks like a patch body but does not explicitly invoke `apply_patch`.

It was rejected because the user scoped this batch to interception first. Implicit-invocation rejection remains a later batch.

## Code Boundaries

### `crates/remote-exec-broker/src/tools/exec_intercept.rs`

- new focused broker-local interception parser
- parse `ExecCommandInput` into either:
  - no interception
  - intercepted patch request with extracted patch body and effective workdir
- recognize the explicit direct, alias, heredoc, and `cd <path> && ...` forms only

### `crates/remote-exec-broker/src/tools/exec.rs`

- call interception before daemon `exec_start`
- on non-match, keep current unified exec behavior
- on match:
  - call the shared broker patch forwarding helper
  - wrap the patch summary into unified-exec-style text and structured output
  - avoid broker session creation

### `crates/remote-exec-broker/src/tools/patch.rs`

- keep direct `apply_patch` behavior unchanged
- expose a small shared forwarding helper for broker-side patch execution so direct tool calls and intercepted exec calls reuse:
  - target lookup
  - identity verification
  - daemon patch RPC

### `crates/remote-exec-broker/src/mcp_server.rs`

- add a dedicated formatter for intercepted patch success
- do not overload `format_command_text` with interception-specific omissions

### `crates/remote-exec-broker/tests/mcp_exec.rs`

- add interception-focused broker integration tests
- keep current session-oriented exec tests unchanged

### `crates/remote-exec-broker/tests/support/mod.rs`

- extend the stub daemon so tests can assert:
  - whether `/v1/exec/start` was called
  - what patch body and workdir reached `/v1/patch/apply`

## Parser Shape

The interception parser should return a small broker-local shape such as:

- `InterceptedApplyPatch`
  - `patch: String`
  - `workdir: Option<String>`

Behavior:

1. start from the incoming `ExecCommandInput.workdir`
2. if the matched shell script includes `cd <path> && ...`, resolve that path relative to the incoming workdir
3. pass the resulting workdir into the broker patch forwarding helper

The parser must not mutate any session state and must not perform daemon calls.

## Data Flow

### Intercepted success path

1. broker receives `exec_command({ target, cmd, workdir?, ... })`
2. broker interception parser matches an explicit patch form
3. broker extracts patch body and effective workdir
4. broker runs shared patch forwarding logic
5. daemon applies the patch through the existing direct patch RPC
6. broker wraps the patch summary into unified-exec-style success text and `CommandToolResult`

### Intercepted failure path

1. broker receives `exec_command({ target, cmd, workdir?, ... })`
2. broker interception parser matches an explicit patch form
3. broker runs shared patch forwarding logic
4. direct patch handling fails during parse, verification, or execution
5. broker surfaces a tool error
6. no daemon `exec_start` call occurs and no session is created

### Non-match path

1. broker receives `exec_command({ target, cmd, workdir?, ... })`
2. interception parser returns no match
3. broker continues through existing daemon `exec_start`
4. all current exec session behavior remains unchanged

## Testing Plan

### Broker interception tests

Add tests to prove:

- direct explicit form `apply_patch "<patch>"` intercepts successfully
- alias form `applypatch "<patch>"` intercepts successfully
- heredoc form intercepts and forwards the extracted patch body
- `cd <path> && apply_patch <<'EOF' ... EOF` forwards the expected effective workdir
- intercepted success returns wrapped unified-exec text without `Command:` or `Chunk ID:`
- intercepted success returns unified-exec-shaped structured content with null session fields
- intercepted requests do not call stub `/v1/exec/start`
- non-matching raw patch text still falls through to the ordinary exec path and returns a live exec session result
- invalid patch in a matched intercepted form surfaces as a tool error rather than a synthetic exec success

### Stub-daemon support

Extend the broker test stub daemon to capture:

- exec-start call count
- last patch-apply request payload

This keeps the assertions end-to-end without introducing a fake broker-only code path.

## Success Criteria

This batch is complete when:

- `exec_command` intercepts the approved explicit `apply_patch` and `applypatch` forms before daemon exec launch
- intercepted requests call the broker patch pathway and do not allocate exec sessions
- intercepted successes return wrapped unified-exec-style text and unified-exec-shaped JSON
- intercepted failures surface as tool errors
- raw patch bodies without explicit `apply_patch` invocation still fall through to normal `exec_command`
- direct `apply_patch` behavior remains unchanged
- no warning/event/freeform/implicit-rejection work is introduced in this batch
