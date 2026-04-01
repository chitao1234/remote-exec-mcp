# Local System Tool Rebuild Notes

This document summarizes how the Codex CLI currently implements four local-system tools inside an inspected Codex source tree:

- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

The goal is not to restate the short tool cards. The goal is to capture the actual behavior needed to rebuild compatible standalone versions, including defaults, state machines, output shapes, approval hooks, event emission, edge cases, and mismatches between declared schemas and runtime payloads.

All source paths below are repo-relative paths from the inspected upstream Codex checkout.

## Primary source files

Tool registration and top-level exposure:

- `codex-rs/core/src/tools/spec.rs`
- `codex-rs/tools/src/local_tool.rs`
- `codex-rs/tools/src/view_image.rs`

`exec_command` and `write_stdin` runtime:

- `codex-rs/core/src/tools/handlers/unified_exec.rs`
- `codex-rs/core/src/unified_exec/mod.rs`
- `codex-rs/core/src/unified_exec/process_manager.rs`
- `codex-rs/core/src/unified_exec/process.rs`
- `codex-rs/core/src/unified_exec/async_watcher.rs`
- `codex-rs/core/src/unified_exec/head_tail_buffer.rs`
- `codex-rs/core/src/tools/runtimes/unified_exec.rs`
- `codex-rs/core/src/unified_exec/errors.rs`
- `codex-rs/core/tests/suite/unified_exec.rs`

`apply_patch` runtime and patch engine:

- `codex-rs/core/src/tools/handlers/apply_patch.rs`
- `codex-rs/core/src/apply_patch.rs`
- `codex-rs/core/src/safety.rs`
- `codex-rs/core/src/tools/runtimes/apply_patch.rs`
- `codex-rs/apply-patch/src/lib.rs`
- `codex-rs/apply-patch/src/parser.rs`
- `codex-rs/apply-patch/src/invocation.rs`
- `codex-rs/apply-patch/src/standalone_executable.rs`
- `codex-rs/apply-patch/tests/suite/tool.rs`

`view_image` runtime:

- `codex-rs/core/src/tools/handlers/view_image.rs`
- `codex-rs/utils/image/src/lib.rs`
- `codex-rs/core/src/original_image_detail.rs`
- `codex-rs/core/tests/suite/view_image.rs`

Shared event and tool output types:

- `codex-rs/core/src/tools/context.rs`
- `codex-rs/core/src/tools/events.rs`
- `codex-rs/protocol/src/protocol.rs`

## Important architectural fact

These tools have more than one representation.

There are four layers:

1. Tool declaration layer.
   This is the schema shown to the model or to external callers. It comes from `ToolSpec`, `ResponsesApiTool`, and freeform-tool declarations.

2. Handler input layer.
   This is the Rust struct actually deserialized by the tool handler. Defaults live here, not in the declaration.

3. Model-facing output layer.
   This is what Codex sends back into the conversation as a `FunctionCallOutput` or `CustomToolCallOutput`.

4. Code-mode or developer-facing output layer.
   This is the JSON-ish object returned by `code_mode_result()` and reflected by tool `output_schema`.

Those layers do not always match exactly.

The two biggest mismatches are:

- `exec_command` and `write_stdin` declare structured JSON output schemas, but normal model-facing responses are plain text blocks with metadata headers.
- `view_image` declares an object output schema `{ image_url, detail }`, but normal model-facing responses are a single `input_image` content item rather than a text or JSON object.

There is another smaller but externally visible mismatch worth preserving:

- direct `apply_patch` returns a plain text summary to the model and an empty object in code mode
- `apply_patch` intercepted through `exec_command` still comes back through the unified-exec output wrapper, so code mode sees unified-exec-shaped JSON instead

If you rebuild these tools outside Codex, decide explicitly whether you want wire compatibility with:

- the tool declaration surface,
- the internal model-facing response surface,
- or both via an adapter.

## Exposure and availability

### `exec_command` and `write_stdin`

These tools are only registered when the shell backend resolves to `ConfigShellToolType::UnifiedExec`.

That depends on:

- feature flags such as `UnifiedExec`
- model shell preferences
- platform constraints
- Windows sandbox compatibility checks

If unified exec is not selected, Codex exposes older shell tools instead and these two tools do not appear.

`exec_command` is marked as supporting parallel tool calls.

`write_stdin` is not.

### `apply_patch`

`apply_patch` is registered when `ToolsConfig.apply_patch_tool_type` is present.

There are two public variants:

- freeform tool variant for modern models
- JSON function-tool variant for models that need structured arguments

The public tool name is always `apply_patch`.

The parser also accepts `applypatch` as a compatibility alias when intercepting shell-style invocations, but that alias is not the official declared tool name.

`apply_patch` is not marked as supporting parallel tool calls.

### `view_image`

`view_image` is registered as a function tool in the normal tool builder and is marked as supporting parallel tool calls.

However, registration is not the final gate. The handler rejects the call at runtime if the current model does not support image inputs.

Review threads explicitly disable `view_image` at a higher level by mutating features before building the tool set.

The optional `detail` parameter only appears in the declared tool schema when both of these are true:

- the model supports `original` image detail
- feature flag `ImageDetailOriginal` is enabled

## Shared event model

These tools emit side-channel events in addition to their direct tool output.

Relevant protocol event types:

- `ExecCommandBeginEvent`
- `ExecCommandOutputDeltaEvent`
- `ExecCommandEndEvent`
- `TerminalInteractionEvent`
- `PatchApplyBeginEvent`
- `PatchApplyEndEvent`
- `ViewImageToolCallEvent`

Those events are important if you want a standalone rebuild that behaves like Codex in a TUI, app-server, or thread-history UI.

## `exec_command`

### Declared interface

Declared by `create_exec_command_tool()` in `codex-rs/tools/src/local_tool.rs`.

Parameters:

- `cmd: string`
- `workdir?: string`
- `shell?: string`
- `tty?: boolean`
- `yield_time_ms?: number`
- `max_output_tokens?: number`
- `login?: boolean` when login shells are allowed by config
- `sandbox_permissions?: string`
- `additional_permissions?: object` when exec-permission approvals are enabled
- `justification?: string`
- `prefix_rule?: string[]`

Required:

- `cmd`

Declared output schema:

- `chunk_id?: string`
- `wall_time_seconds: number`
- `exit_code?: number`
- `session_id?: number`
- `original_token_count?: number`
- `output: string`

### Handler input defaults

Actual handler defaults live in `ExecCommandArgs` inside `codex-rs/core/src/tools/handlers/unified_exec.rs`.

Defaults:

- `tty = false`
- `yield_time_ms = 10000`
- `login = None`
- `max_output_tokens = None`
- `sandbox_permissions = use_default`
- `additional_permissions = None`
- `justification = None`
- `prefix_rule = None`

### Shell command construction

The tool does not execute `cmd` directly.

It converts `cmd` into a shell invocation through `get_command()`.

Behavior:

- If `shell` is omitted, Codex uses the session's current user shell.
- If `shell` is supplied, Codex creates a shell description from that path or executable name and uses it instead.
- In direct mode, the shell implementation decides how to derive argv. For example, a Unix shell typically becomes something like `["/bin/zsh", "-lc", "<cmd>"]`.
- In zsh-fork mode, the tool ignores the model-supplied `shell` override and always uses the configured zsh bridge executable plus either `-lc` or `-c`.

Login-shell behavior:

- `login = true` explicitly asks for login-shell semantics.
- `login = false` explicitly disables them.
- If `login` is omitted, the effective value defaults to the session-level `allow_login_shell` setting.
- If the caller explicitly sets `login = true` but login shells are disabled by config, the tool rejects the call with a model-visible error.

### Working directory resolution

There are two separate uses of cwd:

- parsing relative permission paths
- process launch cwd

Parsing flow:

- the handler first resolves a base path from the raw JSON arguments and the turn cwd
- then it parses `ExecCommandArgs` relative to that base path
- relative paths in `additional_permissions` are resolved against the effective workdir, not just the turn cwd

Execution flow:

- if `workdir` is absent, the process uses the turn cwd
- if `workdir` is present and non-empty, it is resolved relative to the turn cwd

### Approval and permission handling

`exec_command` is tightly integrated with Codex approval logic.

It carries three related permission concepts:

- `sandbox_permissions`
- `additional_permissions`
- sticky or previously granted turn permissions

High-level behavior:

- granted turn permissions are merged into the request before validation
- relative additional-permission paths are normalized against the resolved cwd
- the request may be rejected if it asks for escalation while the approval policy does not allow asking
- `prefix_rule` is used by exec-policy logic to cache approvals for command families
- `justification` is surfaced to approval UI or guardian review

Standalone rebuild note:

If you do not need Codex's approval model, you can omit this layer. If you want behavioral compatibility, you need:

- approval caching keyed by canonicalized command, cwd, tty flag, sandbox mode, and additional permissions
- a policy engine that can request approval before spawn
- retry semantics for sandbox failures

### Special interception: `apply_patch` inside `exec_command`

Before launching a process, `exec_command` checks whether the command is actually an `apply_patch` invocation disguised as shell text.

If so:

- the command is not executed as unified exec
- the `apply_patch` pathway runs instead
- the reserved process id is released
- the user-visible tool result becomes the patch result
- the response does not include `Command:`, `Chunk ID:`, or a session id because those fields are left empty
- however, the intercepted result is still wrapped as an `ExecCommandToolOutput`, so the normal unified-exec formatter still emits `Wall time: 0.0000 seconds` and `Output:`

This interception exists to steer the model toward using `apply_patch` as a first-class tool.

The handler also records a model warning saying that `apply_patch` should be used directly rather than through `exec_command`.

### Process model

`exec_command` uses the unified exec process manager.

The process manager:

- allocates a numeric process id before launch
- launches a process under approval and sandbox orchestration
- optionally keeps the process alive for later `write_stdin` calls
- stores live sessions in memory
- prunes old sessions when the limit is exceeded

Important constants from `codex-rs/core/src/unified_exec/mod.rs`:

- minimum yield time: `250 ms`
- minimum empty-poll yield time: `5000 ms`
- maximum normal yield time: `30000 ms`
- default max background poll window: `300000 ms`
- default max output tokens for truncation: `10000`
- retained transcript cap: `1 MiB`
- maximum live unified exec processes: `64`
- warning threshold for open processes: `60`

### PTY versus pipes

The public description says "Runs a command in a PTY", but the implementation is more specific.

- `tty = true` launches an interactive PTY-backed process.
- `tty = false` launches a pipe-backed process with no writable stdin path.

This difference matters for `write_stdin`.

### Environment construction

Unified exec does not inherit the raw parent environment unchanged.

The process manager overlays a fixed environment intended to reduce terminal noise and pager behavior:

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

Additional environment behavior:

- shell environment policy contributes env vars through `create_env()`
- explicit shell env overrides from turn config are preserved
- network-proxy configuration can inject proxy environment variables
- PowerShell commands are prefixed for UTF-8 correctness
- in zsh-fork mode, Codex may prepare a specialized exec request instead of using the direct path

### Local versus remote execution backend

The process manager supports two spawn backends:

- local spawn via PTY or pipes
- remote exec-server spawn when `exec_server_url` is configured

Remote backend constraints:

- inherited file descriptors are not supported
- writes and reads go through the exec-server protocol

### Yield semantics

After launch, `exec_command` does not necessarily wait for process completion.

Instead it:

1. starts the process
2. emits an `ExecCommandBegin` event
3. starts background output streaming
4. waits up to the requested yield deadline for initial output
5. returns either:
   - a completed result if the process exited quickly
   - or a live session id if it is still running

For `exec_command`, the requested yield is clamped to `[250 ms, 30000 ms]`.

### Output buffering and truncation

Unified exec maintains two output views:

1. A per-call collected snapshot returned to the tool caller.
2. A longer-lived in-memory transcript for background sessions and end events.

Transcript retention uses a symmetric head-tail buffer:

- first half of capacity keeps the earliest bytes
- second half keeps the latest bytes
- middle bytes are dropped once the 1 MiB cap is exceeded

This means long-running sessions preserve prefix and suffix, not the full middle.

Live delta streaming:

- unified exec emits `ExecCommandOutputDelta` events as UTF-8-safe chunks
- each emitted delta chunk is capped to `8192` bytes
- at most `10000` output-delta events are emitted per call
- output is tagged as `stdout` in deltas

Trailing-output behavior:

- after exit, the async watcher waits a short grace period of `100 ms` to capture trailing bytes before finalizing the session

### Pause-aware waiting

While the session is in an out-of-band elicitation paused state, unified exec extends its deadlines instead of letting the timeout expire during the pause.

That affects both:

- the initial `exec_command` wait
- later `write_stdin` polls

### Response format returned to the model

The normal model-facing response is plain text assembled by `ExecCommandToolOutput::response_text()`.

It includes:

- `Command: ...` if the session command is known
- `Chunk ID: ...`
- `Wall time: ... seconds`
- `Process exited with code ...` when available
- `Process running with session ID ...` when the process is still alive
- `Original token count: ...` when computed
- `Output:`
- truncated command output

The output text is token-truncated using the requested `max_output_tokens` or the default token limit.

Important success/failure semantic:

- a process exiting with a non-zero exit code does not make the tool call itself fail
- the tool call still succeeds and reports the exit status inside the formatted payload or code-mode JSON
- tool-call failure is reserved for handler-level failures such as process creation failure, sandbox denial, approval rejection, unknown session id, or similar orchestration/runtime errors

The tool declaration's JSON output schema is instead produced by `code_mode_result()`, not by the normal model-facing function output.

There is also a separate post-tool-use surface used by some higher-level flows:

- for completed non-background sessions, `post_tool_use_response()` returns only the truncated raw command output string
- for still-running sessions, it returns nothing
- for intercepted `apply_patch`, it returns nothing because the wrapped result has no `session_command`

### Result metadata rules

If the process is still alive at return time:

- `process_id` is returned
- `exit_code` is usually absent

If the process has completed:

- `process_id` is omitted
- `exit_code` is included

`chunk_id`:

- is a random 6-character lowercase hex string
- is generated per tool response, not per process

`original_token_count`:

- is an approximate token count computed before truncation

### Event behavior

Startup:

- `ExecCommandBeginEvent`
- source is `unified_exec_startup`
- includes `call_id`, `process_id`, `turn_id`, full command argv, cwd, parsed command actions, and no `interaction_input`

Streaming:

- `ExecCommandOutputDeltaEvent`
- keyed by the original `exec_command` call id

Completion:

- `ExecCommandEndEvent`
- may arrive after the tool already returned if the process continued in the background
- includes stdout, stderr, aggregated output, exit code, duration, formatted output, and status

Statuses:

- `completed`
- `failed`
- `declined`

### Session persistence and pruning

A running process is inserted into the process store immediately after startup.

Pruning policy:

- hard limit is 64 open processes
- the 8 most recently used processes are protected
- pruning prefers exited sessions first
- otherwise it prunes the least recently used non-protected session
- pruned processes are terminated

### Errors and edge cases

Notable errors:

- explicit login shell requested while disabled
- process creation failure
- sandbox denial
- background session later becoming unknown
- process failure detected after startup

Non-obvious behavior:

- a non-interactive command can still be kept as a live session if it has not exited yet
- such a session can be polled later with `write_stdin(chars="")`
- but it cannot accept non-empty stdin unless it was launched with `tty=true`

## `write_stdin`

### Declared interface

Declared by `create_write_stdin_tool()` in `codex-rs/tools/src/local_tool.rs`.

Parameters:

- `session_id: number`
- `chars?: string`
- `yield_time_ms?: number`
- `max_output_tokens?: number`

Required:

- `session_id`

Declared output schema is the same as `exec_command`.

### Handler input defaults

Actual handler defaults:

- `chars = ""`
- `yield_time_ms = 250`
- `max_output_tokens = None`

Important naming note:

- public API says `session_id`
- internal state uses `process_id`
- error text refers to "process id"

### Core behavior

`write_stdin` locates an existing unified exec session and optionally does two things:

1. writes bytes to stdin
2. polls for new output until a deadline

If `chars` is empty:

- no bytes are written
- the call acts as a poll operation

If `chars` is non-empty:

- bytes are written to the process first
- then Codex waits for output

### TTY requirement

Non-empty writes require the original process to have been launched with `tty=true`.

If `chars` is non-empty and the process is non-TTY:

- the tool returns `stdin is closed for this session; rerun exec_command with tty=true to keep stdin open`

Empty polls do not require TTY and can be used to observe output from non-interactive sessions that remain alive.

### Yield-time rules

`write_stdin` uses two different clamp policies.

For empty polls:

- lower bound: `5000 ms`
- upper bound: `background_terminal_max_timeout`
- default upper bound if unspecified in config: `300000 ms`

For non-empty writes:

- lower bound: `250 ms`
- upper bound: `30000 ms`

This means the declared default of `250` is only the starting point. An empty poll is silently widened to at least 5 seconds.

### Response behavior

The returned object is the same internal type used by `exec_command`.

Important fields:

- `event_call_id` is the original `exec_command` call id, not the current `write_stdin` call id
- `process_id` is present while the process is still alive
- `exit_code` appears once the process has terminated
- `session_command` is copied from the original session

As with `exec_command`, the model-facing output is a formatted plain-text block, not a JSON object.

### Terminal interaction events

After a successful `write_stdin`, the handler emits a `TerminalInteractionEvent`.

Critical detail:

- the event `call_id` is the original `exec_command` call id associated with the session
- not the `write_stdin` call id

This allows UIs to associate terminal input with the original command session.

The event contains:

- `call_id`
- `process_id`
- `stdin`

Important polling nuance:

- this event is emitted even when `chars` is empty
- in that case the event still appears, with `stdin = ""`
- so a pure poll is observable in the event stream as a terminal interaction, not just as silent output collection

### Session lifecycle on exit

After polling, `write_stdin` refreshes session state.

If the process exited:

- the response omits `process_id`
- the response includes `exit_code`
- the session is removed from the process store

Further calls with the old `session_id` then fail with `Unknown process id <id>`.

### Error cases

Notable errors:

- unknown session id
- stdin closed because the session was non-TTY
- write failure due to process death
- process failure message surfaced by the runtime

Tool-surface note:

- the handler wraps underlying errors as `write_stdin failed: <error>`
- so callers typically see the wrapped message, not just the bare unified-exec error text

## `apply_patch`

### Public tool variants

There are two declared variants under the same tool name.

Freeform variant:

- tool name: `apply_patch`
- payload is raw patch text
- grammar is Lark

JSON function variant:

- tool name: `apply_patch`
- input shape is `{ "input": "<entire patch text>" }`

Both route to the same handler, but they reach it through different payload shapes:

- freeform tool calls arrive as `ToolPayload::Custom { input }`
- JSON function-tool calls arrive as `ToolPayload::Function { arguments }` and are deserialized into `ApplyPatchToolArgs { input }`

In both cases the handler extracts a single patch string and then re-verifies it using the same `codex-apply-patch` parser before any mutation or approval flow.

### Freeform grammar

The freeform grammar loaded from `tool_apply_patch.lark` is:

```lark
start: begin_patch hunk+ end_patch
begin_patch: "*** Begin Patch" LF
end_patch: "*** End Patch" LF?

hunk: add_hunk | delete_hunk | update_hunk
add_hunk: "*** Add File: " filename LF add_line+
delete_hunk: "*** Delete File: " filename LF
update_hunk: "*** Update File: " filename LF change_move? change?

filename: /(.+)/
add_line: "+" /(.*)/ LF -> line

change_move: "*** Move to: " filename LF
change: (change_context | change_line)+ eof_line?
change_context: ("@@" | "@@ " /(.+)/) LF
change_line: ("+" | "-" | " ") /(.*)/ LF
eof_line: "*** End of File" LF
```

The Rust parser is slightly more lenient than the grammar comment suggests:

- it tolerates some leading or trailing whitespace around patch markers
- it accepts a few shell-wrapper forms during interception and verification
- it is the runtime parser that ultimately decides what is accepted, not the Lark declaration alone

This distinction matters for rebuilds:

- the declared freeform grammar is what models or external callers see
- the actual mutation semantics come from `codex-apply-patch` parsing and verification
- reproducing only the Lark grammar will not exactly match Codex acceptance or rejection behavior

### Accepted invocation forms

The parser recognizes:

1. Direct invocation:
   `apply_patch "<patch>"`

2. Alias direct invocation:
   `applypatch "<patch>"`

3. Shell-script heredoc form:
   optional `cd <path> &&`
   then `apply_patch <<'PATCH' ... PATCH`

Shell wrappers accepted by the interception logic:

- Unix shells with `-lc` or `-c`
- PowerShell or pwsh with `-Command` and optional `-NoProfile`
- `cmd /c`

Important limitation:

- even for PowerShell and cmd, the extraction logic still expects a very narrow bash-like heredoc script structure
- this is not a general-purpose shell parser

More precisely, the shell-wrapper matcher is intentionally conservative:

- the wrapper must look like exactly one top-level shell statement
- the accepted forms are effectively:
  - `apply_patch <<'EOF' ... EOF`
  - `cd <path> && apply_patch <<'EOF' ... EOF`
- the `cd` form must use `&&`, not `;`, `||`, or a pipe
- the `cd` command must have exactly one positional path argument
- trailing commands like `&& echo done` or preceding commands like `echo x; apply_patch ...` do not match
- `apply_patch` and `applypatch` are both accepted command names at this parsing layer

That means a standalone rebuild needs either:

- the same narrow wrapper parser,
- or a consciously broader parser that will diverge from Codex on edge cases

### Implicit-invocation rejection

Codex intentionally rejects raw patch bodies that are not explicitly wrapped as an `apply_patch` call.

Rejected examples:

- argv is a single string that is itself a patch
- shell script body is itself a patch with no explicit `apply_patch`

The error is:

- `patch detected without explicit call to apply_patch. Rerun as ["apply_patch", "<patch>"]`

This behavior matters if you want compatibility with Codex's model training and approval flow.

### Verification before execution

The handler does not trust the raw tool payload.

It always re-parses and verifies the patch with `maybe_parse_apply_patch_verified()`.

Verification produces an `ApplyPatchAction`:

- `patch`: the raw patch body
- `cwd`: effective cwd used to resolve relative paths
- `changes`: a map of absolute target paths to semantic changes

For update hunks, verification computes:

- the new file content that would result
- a unified diff relative to the current file on disk

For delete hunks, verification reads and records the deleted file's current contents.

Important implementation detail:

- direct tool calls are verified by constructing argv equivalent to `["apply_patch", patch_input]`
- intercepted shell-style invocations are verified from the parsed shell argv or extracted heredoc body
- verification is where implicit raw patch bodies are rejected, effective cwd is chosen, and semantic file changes are computed

Text-model limitation:

- verification for update and delete hunks uses UTF-8 text reads
- non-UTF-8 files therefore do not round-trip through this pathway cleanly
- `apply_patch` is a text-file editing tool, not a binary patch tool

### Relative path resolution

Patch file paths are resolved against an effective cwd.

Effective cwd rules:

- direct tool calls use the turn cwd
- heredoc shell wrappers may include a single `cd <path> && ...`
- if that `cd` path is relative, it is resolved against the original cwd
- if it is absolute, it becomes the effective cwd directly

Move destinations in update hunks are also resolved against that effective cwd.

### Safety and approval logic

After verification, Codex evaluates patch safety in `assess_patch_safety()`.

Possible outcomes:

- auto-approve
- ask user
- reject immediately

Key rules:

- empty patches are rejected
- if every affected path is within writable roots, the patch may be auto-approved
- `OnFailure` approval policy also allows auto-approval attempts
- if the outer sandbox policy is `DangerFullAccess` or `ExternalSandbox`, auto-approved patches run unsandboxed
- otherwise Codex prefers to auto-approve only when a platform sandbox exists
- if sandbox approval is disallowed by approval settings and the patch is outside writable roots, the patch is rejected
- `UnlessTrusted` currently short-circuits to `AskUser`

Extra permissions:

- Codex computes file-system write permissions for parent directories of affected files that are outside current writable roots
- those permissions are normalized into a `PermissionProfile`
- granted turn permissions are merged in before final orchestration

Approval caching and runtime review are a separate layer from `assess_patch_safety()`:

- `assess_patch_safety()` decides whether the patch is auto-approved, must ask the user, or is rejected immediately
- if execution proceeds through the runtime, approval caching is keyed by the absolute affected file paths, not by the raw patch text
- preapproved additional permissions can short-circuit the runtime approval prompt on the first attempt

That means two different patches touching the same file set can reuse approval cache entries in ways that a naive patch-string cache would not.

### Execution strategy

If the patch is rejected by safety checks:

- the tool returns a model-visible error and does not execute

In the normal successful case:

- Codex delegates actual filesystem mutation to an exec runtime
- the runtime self-invokes the Codex binary with `--codex-run-as-apply-patch`
- argv becomes:
  - `["<codex-exe>", "--codex-run-as-apply-patch", "<raw-patch>"]`
- cwd is the patch action cwd
- environment is intentionally empty for determinism

That runtime still participates in orchestrator-managed approval and sandbox selection.

There are two main execution entry paths:

1. Direct `apply_patch` tool call
   - handler verifies the patch
   - handler computes effective permissions and emits patch events
   - runtime self-invokes Codex with `--codex-run-as-apply-patch`

2. `exec_command` interception
   - unified exec first recognizes the command as a disguised `apply_patch`
   - reserved unified-exec process id is released
   - patch events are emitted instead of unified-exec begin/end events
   - the `exec_command` yield timeout is forwarded into the apply-patch runtime request

That second path is an important behavioral match if you want Codex-like interception rather than treating `apply_patch`-through-shell as a normal subprocess.

In the current code, the handler's non-exec `Output(...)` branch is effectively only used for immediate error returns rather than for successful in-process patch application.

### Standalone CLI behavior

The standalone executable in `standalone_executable.rs` behaves like this:

- if exactly one UTF-8 argument is present, use it as the patch
- if no argument is present, read the patch from stdin
- if stdin is empty, print usage and exit with code 2
- if there are extra arguments, print an error and exit with code 2
- parse and apply the patch
- exit 0 on success
- exit 1 on parse or apply failure

Usage text:

```text
apply_patch 'PATCH'
echo 'PATCH' | apply_patch
```

### Filesystem mutation semantics

This is the most important part for a rebuild.

All patch application is text-oriented:

- files are read and rewritten as UTF-8 text
- chunk matching operates on logical lines
- summary output is derived from semantic file actions rather than from raw byte diffs

#### Add file

Behavior:

- creates parent directories if needed
- writes the provided contents
- overwrites any existing file at that path
- reports the path as added

This means `*** Add File:` is not "create-if-absent". It is effectively "write this full file and classify it as add".

#### Delete file

Behavior:

- removes the file with `remove_file`
- fails if the path does not exist
- fails if the path is a directory
- records the deleted file contents during verification
- uses text reads during verification, so non-UTF-8 files can fail before removal

#### Update file

Behavior:

- source file must already exist and be readable
- source file must be readable as UTF-8 text
- patch chunks are matched against current file content
- replacements are computed in memory before write
- if `*** Move to:` is present:
  - parent directories for the destination are created
  - destination file is written
  - original file is removed
  - any existing destination file is overwritten
- if there is no move:
  - source file is overwritten in place

Trailing newline behavior:

- updated files are forced to end with a trailing newline

#### Matching semantics

Update chunks use a custom matching algorithm:

- optional `@@ context` lines narrow the search window
- old lines must appear in order after the context anchor
- if a chunk ends with an empty line sentinel, the matcher retries without that sentinel to handle EOF edits
- replacements are sorted and then applied from the end of the file backward
- a missing or empty update body is rejected before any filesystem write
- invalid top-level hunk headers are rejected with explicit parse errors
- path handling is strictly relative inside the patch language even though effective cwd may come from the surrounding invocation

If expected context cannot be found, the patch fails.

#### Atomicity

`apply_patch` is not atomic across multiple hunks or files.

If one operation succeeds and a later one fails:

- earlier changes remain on disk
- no rollback is attempted

This is explicitly covered by tests.

### Success output

On success, `apply_patch` prints a git-style summary to stdout:

```text
Success. Updated the following files:
A path
M path
D path
```

That summary text is what the tool returns to the model.

### Failure output

Failure text goes to stderr and is surfaced back to the model as an error.

Examples:

- invalid patch markers
- invalid hunk headers
- empty patch
- update hunk with no changes
- missing context during update
- missing file on update
- delete of missing file
- delete of a directory
- non-UTF-8 direct argument
- invalid implicit invocation without explicit `apply_patch`
- non-UTF-8 or otherwise unreadable text files during verification/update paths

Tool-surface vs runtime-surface distinction:

- successful tool calls return the summary text to the model
- failed tool calls are surfaced as model-visible errors rather than success payloads containing stderr text
- `code_mode_result()` for successful `apply_patch` still returns an empty JSON object even though the model-facing response is summary text

### Event behavior

`apply_patch` emits:

- `PatchApplyBeginEvent`
- `PatchApplyEndEvent`

Those events include the semantic `changes` map, not just raw text.

The event payloads are built from verified semantic file changes:

- add events carry full added file content
- delete events carry deleted file content captured during verification
- update events carry unified diffs and optional move destinations

This means event consumers can reconstruct the intended change set more precisely than they could from the summary text alone.

Statuses:

- `completed`
- `failed`
- `declined`

When `exec_command` interception redirects a disguised patch into `apply_patch`, Codex emits patch events and deliberately does not emit unified-exec begin or end events for that operation.

### Output representation mismatch

Normal model-facing response:

- plain text summary on success

Code-mode response:

- empty JSON object

Interception caveat:

- when `apply_patch` is invoked through `exec_command` interception, the normal model-facing payload still contains the patch summary text
- but code mode does not see the empty-object `apply_patch` shape
- instead it sees unified-exec JSON with fields like `wall_time_seconds` and `output`, because the intercepted result is wrapped as `ExecCommandToolOutput`

That mismatch is intentional in the current implementation.

## `view_image`

### Declared interface

Declared by `create_view_image_tool()` in `codex-rs/tools/src/view_image.rs`.

Always present:

- `path: string`

Conditionally present:

- `detail?: string`

The `detail` parameter only exists in the declaration when original-detail support is enabled for both features and model capability.

Declared output schema:

- `image_url: string`
- `detail: string | null`

### Runtime gating

Even if the tool is registered, the handler first checks whether the current model supports image inputs.

If not, the call fails with:

- `view_image is not allowed because you do not support image inputs`

### Path resolution and validation

The handler:

1. parses `path` and optional `detail`
2. resolves the path relative to the turn cwd
3. converts it to an absolute path type
4. fetches filesystem metadata
5. requires that the target exists and is a file
6. reads the entire file into memory

Behavior note:

- although the tool description tells models to use a full local filepath, the runtime also accepts relative paths and resolves them against the turn cwd

Error cases:

- bad path resolution
- missing file
- path exists but is a directory
- file is not a decodable image

### `detail` semantics

Accepted values at the handler level:

- omitted
- `null`
- `"original"`

Rejected values:

- anything else, for example `"low"`

Error message:

- `view_image.detail only supports `original`; omit `detail` for default resized behavior, got `<value>``

Important capability rule:

- `"original"` is only honored when both the feature flag and the model capability are present
- if either one is missing, Codex silently falls back to resized behavior

`null` is treated the same as omission.

### Image processing rules

The actual image processing lives in `codex-rs/utils/image/src/lib.rs`.

Relevant constants:

- max width for resized images: `2048`
- max height for resized images: `768`

Modes:

- `ResizeToFit`
- `Original`

Default mode:

- `ResizeToFit`

If `Original` is active:

- original resolution is preserved
- no resize occurs

If `ResizeToFit` is active:

- images already within `2048x768` are not resized
- larger images are resized to fit within those bounds using `FilterType::Triangle`

Input format handling:

- recognized passthrough formats: PNG, JPEG, WebP
- GIF is recognized as an input format but is not preserved byte-for-byte
- unsupported formats fail during decode or processing

Encoding behavior:

- if no resize is needed and the source format is PNG, JPEG, or WebP, Codex reuses the original file bytes directly
- otherwise it re-encodes:
  - PNG for the generic fallback
  - JPEG at quality 85 when JPEG is the chosen output format
  - lossless WebP when WebP is the chosen output format

Output is returned as a `data:<mime>;base64,...` URL.

The helper also caches processed images in an LRU cache keyed by SHA-1 digest plus mode. That cache improves performance but is not required for protocol compatibility.

### Actual model-facing output

This is another place where the runtime differs from the declared object schema.

Normal model-facing output is not a JSON object.

Instead, the handler returns a function-call output whose body is a single `input_image` content item:

- `image_url`
- optional `detail = original`

There is no extra text label.

There is no separate synthetic image message inserted outside the tool output.

When default detail is used, the `detail` field is omitted from the content item rather than serialized as `null`.

### Developer-facing or code-mode output

`code_mode_result()` returns:

- `{ "image_url": "...", "detail": <detail-or-null> }`

That shape matches the declared output schema more closely than the normal model-facing output does.

### Event behavior

On success, the handler emits:

- `ViewImageToolCallEvent { call_id, path }`

The event only records the path and the originating tool call id.

It does not include the data URL payload.

### Error behavior

Typical error strings:

- missing file:
  `unable to locate image at `<abs-path>`: ...`
- directory path:
  `image path `<abs-path>` is not a file`
- non-image file:
  `unable to process image at `<abs-path>`: unsupported image ...`
- text-only model:
  `view_image is not allowed because you do not support image inputs`

Failure-shape behavior:

- on failure, `view_image` produces ordinary text tool output rather than an `input_image` content item
- no image content item is emitted on error
- no separate synthetic image message is inserted on error, just as on success

## Rebuild checklist

If you want a standalone rebuild that behaves like Codex rather than just "similar enough", preserve these details:

- `exec_command` and `write_stdin` must share a session store and reuse the same process ids.
- `exec_command` must be able to return early with a live session id instead of waiting for completion.
- `write_stdin(chars="")` must act as a poll.
- Non-empty `write_stdin` must require a TTY-backed session.
- Unified exec transcript retention should be head-tail, not just tail.
- `exec_command` should intercept disguised `apply_patch` invocations.
- `apply_patch` verification must run before mutation and must reject implicit raw patches.
- `apply_patch` must not promise atomic rollback; partial success is observable behavior.
- `*** Add File:` should overwrite existing files, because the current implementation does.
- `*** Move to:` should overwrite existing destinations, because the current implementation does.
- `view_image` should return a data URL and use resize-to-fit by default.
- `view_image` should only honor `"original"` when both feature and model capability allow it.
- If you need Codex-like UI integration, replicate the begin, delta, end, interaction, patch, and image-view events.

## Recommended standalone interface choice

If building from scratch, the cleanest external interface is probably:

- keep the current input schemas
- return structured JSON for all four tools externally
- internally preserve the event model and session behavior

If instead you need behavioral fidelity to the current Codex model loop, preserve the current mismatches:

- `exec_command` and `write_stdin` return formatted text to the model
- `apply_patch` returns text summary on success
- `view_image` returns an `input_image` content item rather than plain JSON

That distinction is the single biggest source of confusion when reading the source for these tools.
