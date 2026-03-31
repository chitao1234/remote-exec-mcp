# Remote MCP Design For Multi-Machine Local-System Tools

Status: approved design captured in writing

Date: 2026-03-31

References:

- [Local System Tool Rebuild Notes](../local-system-tools.md)
- Inspected Codex source reference: `codex-rs/docs/codex_mcp_interface.md`

## Goal

Build a standalone MCP server that exposes remote equivalents of the current local-system tools:

- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

The new server must let an agent operate on multiple Linux machines, not just the broker's local machine, while staying close to the current tool interface and behavior.

The main compatibility target is behavioral compatibility with the documented tool semantics in [local-system-tools.md](../local-system-tools.md), not a byte-for-byte clone of Codex internals.

## Approved Constraints

These design choices were already selected before writing this document:

- Use a per-machine daemon, not direct SSH execution.
- Linux only for v1.
- Keep the same public tool names.
- Add arguments rather than creating tool namespaces.
- Use a simpler `danger-full-access` trust model.
- Do not reproduce Codex's interactive approval flow or sandbox-escalation model.
- Prefer a cleaner remote-first MCP API when it materially improves correctness, as long as the interface stays close to the existing one.

## Scope

This design covers:

- the public MCP tool surface
- the broker and per-machine daemon split
- session routing for `exec_command` and `write_stdin`
- how the local-tool spec maps onto remote behavior
- failures, observability, and testing

This design does not cover:

- Windows or macOS targets
- sandboxing or approval prompts
- session persistence across broker restart
- session persistence across daemon restart
- fleet orchestration, scheduling, or load balancing
- a general remote filesystem API beyond what these four tools need

## Reuse Strategy From The Local Tool Spec

The earlier tool document remains the semantic source of truth for what each tool should do once execution reaches a single target machine.

Use that document in the following way:

- `exec_command` and `write_stdin`
  - Preserve PTY versus pipe behavior.
  - Preserve yield-and-return semantics.
  - Preserve polling via `write_stdin(chars="")`.
  - Preserve the rule that non-empty stdin writes require `tty=true`.
  - Preserve transcript retention and truncation behavior as closely as practical.
  - Preserve the distinction between launch failure and normal nonzero exit.
- `apply_patch`
  - Preserve the patch grammar and verification rules.
  - Preserve current file mutation semantics, including overwrite behavior for `*** Add File:` and `*** Move to:`.
  - Preserve the current non-atomic multi-file behavior.
  - Preserve explicit-call requirements rather than accepting raw patch bodies implicitly.
- `view_image`
  - Preserve relative path resolution from a working directory.
  - Preserve resize-to-fit default behavior.
  - Preserve `"original"` as the only special `detail` value.
  - Preserve data URL output.

What changes in the remote version is primarily the transport and trust boundary:

- target selection becomes explicit
- sessions become broker-routed
- sandbox and approval concerns are removed

## Chosen Architecture

Use a thin public MCP broker and a richer per-machine daemon.

### Broker responsibilities

The broker is the only component exposed to the agent as an MCP server. It is responsible for:

- publishing the four public tools
- validating public tool arguments
- resolving the target machine
- forwarding requests to the correct daemon
- allocating and tracking public session IDs
- enforcing that `write_stdin` only reaches the daemon that owns the session
- translating daemon responses into the public MCP tool result format
- surfacing predictable errors for unknown targets, unknown sessions, and disconnected daemons

The broker should stay narrow. It should not re-implement local execution semantics itself.

### Daemon responsibilities

Each target machine runs one daemon. The daemon is responsible for:

- spawning and tracking local processes
- implementing PTY and non-PTY execution
- handling stdin writes and polling for existing sessions
- applying verified patches locally
- reading and transforming local images
- enforcing Linux-only local assumptions
- exposing a machine-local session store

The daemon is where most of the behavior from [local-system-tools.md](../local-system-tools.md) should live.

### Why this split

This split keeps the broker simple and makes target-local behavior correct.

If the broker tried to own process state or patch/image behavior centrally, it would either need to duplicate local-system logic or invent a more complicated cross-machine protocol. A richer daemon avoids that and keeps machine-local semantics close to the filesystem and process model they act on.

## Trust And Security Model

The target machine is the trust boundary.

Selecting a target is equivalent to selecting a `danger-full-access` execution environment on that machine. Once a request is routed to a trusted daemon:

- the daemon may read and write any file the daemon user can access
- the daemon may spawn arbitrary processes the daemon user can execute
- there is no per-call approval prompt
- there is no sandbox selection or escalation flow

This deliberately removes several arguments from the local `exec_command` surface:

- `sandbox_permissions`
- `additional_permissions`
- `justification`
- `prefix_rule`

Those fields exist to support Codex's local approval and sandbox system. They do not fit the remote-first trust model and should not be part of the v1 public API.

Security in v1 should focus on machine-to-machine trust, not per-call approvals:

- broker-to-daemon communication should use mutually authenticated TLS
- each daemon should present a stable machine identity
- each configured `target` should map to one expected daemon identity

## Target Model

A `target` is a broker-configured machine alias such as `builder-01` or `staging-api`.

For v1:

- targets are statically configured on the broker
- each target maps to exactly one daemon endpoint
- a daemon serves exactly one machine

Target discoverability should not require a new public tool. Instead:

- the broker should include the configured target names in the tool descriptions
- when practical, the broker should advertise `target` as an enum in each tool schema

If the target list is too dynamic for an enum, keep `target` as a free string and validate it server-side.

## Working Directory Model

The local Codex tools always have a turn cwd. The remote server needs an equivalent rule.

For v1:

- each target should have a configured default working directory
- if a tool call provides `workdir`, resolve it on the target relative to that target's default working directory unless it is already absolute
- if a tool call omits `workdir`, use the target's default working directory as the effective cwd

This rule should be used consistently by:

- `exec_command`
- `apply_patch`
- `view_image`

`write_stdin` does not need its own cwd because it always attaches to an existing session.

## Public MCP Tool Surface

The tool names stay the same. The main public change is adding `target` to the tools that operate directly on a machine-local resource.

The broker should return standard MCP `CallToolResult` payloads with:

- `content` for compatibility with current tool behavior
- `structuredContent` for a cleaner machine-readable surface

This keeps the visible behavior close to Codex while giving the standalone MCP server a stable structured contract.

### `exec_command`

Public input:

- `target: string` required
- `cmd: string` required
- `workdir?: string`
- `shell?: string`
- `tty?: boolean`
- `yield_time_ms?: number`
- `max_output_tokens?: number`
- `login?: boolean`

Removed from the remote API:

- `sandbox_permissions`
- `additional_permissions`
- `justification`
- `prefix_rule`

If a caller still sends any of those removed fields, the broker should reject the request with a validation error rather than silently ignoring them.

Public structured result:

- `chunk_id?: string`
- `wall_time_seconds: number`
- `exit_code?: number`
- `session_id?: string`
- `original_token_count?: number`
- `output: string`
- `target: string`

Public `content` should remain close to the current local tool:

- plain-text summary block
- include command, chunk ID, wall time, exit or running status, and output text

Behavior:

- `target` chooses which daemon executes the command
- daemon implements local shell construction, cwd resolution, PTY selection, yield timing, transcript buffering, and exit handling
- if the process completes inside the yield window, return a completed result without `session_id`
- if the process is still running, return an opaque broker-owned `session_id`

### `write_stdin`

Public input:

- `session_id: string` required
- `chars?: string`
- `yield_time_ms?: number`
- `max_output_tokens?: number`
- `target?: string`

The `target` field is optional. It is not needed for routing, because the broker owns the session mapping. If present, the broker should verify that it matches the session's owning target and fail if it does not.

Public structured result:

- `chunk_id?: string`
- `wall_time_seconds: number`
- `exit_code?: number`
- `session_id?: string`
- `original_token_count?: number`
- `output: string`
- `target: string`

Behavior:

- `session_id` is resolved by the broker to `(target, daemon_session_id, owner)`
- non-empty `chars` still requires that the original session was started with `tty=true`
- `chars=""` remains a poll operation
- once the process exits, the response omits `session_id` and includes `exit_code`
- after exit, later calls with the same `session_id` fail cleanly

### `apply_patch`

Public input:

- `target: string` required
- `input: string` required
- `workdir?: string`

The public MCP surface for the standalone server should use structured JSON input rather than a freeform custom-tool grammar. The patch body itself remains plain text and must preserve the current patch grammar.

Public structured result:

- `target: string`
- `output: string`

Public `content`:

- plain-text success summary on success
- tool error on failure

Behavior:

- daemon verifies and applies the patch locally using the current grammar and mutation semantics
- `workdir` supplies the base directory used for relative path resolution on the target machine
- no sandbox or approval logic is applied in v1

Compatibility note:

- the daemon should preserve the current patch parser behavior as documented in [local-system-tools.md](../local-system-tools.md)
- the broker does not need to support shell-wrapper interception, because this API is a first-class `apply_patch` tool, not a shell command parser

### `view_image`

Public input:

- `target: string` required
- `path: string` required
- `workdir?: string`
- `detail?: string | null`

Accepted `detail` values:

- omitted
- `null`
- `"original"`

Any other `detail` value should be rejected.

Public structured result:

- `target: string`
- `image_url: string`
- `detail: string | null`

Public `content`:

- one `input_image` content item containing the data URL
- omit `detail` when default resized behavior is used
- include `detail: "original"` only when explicitly requested and honored

Behavior:

- daemon resolves `path` relative to the effective target cwd defined in the working-directory model above
- daemon validates that the path exists and is a file
- daemon reads and transforms the image locally
- default behavior remains resize-to-fit
- `"original"` remains the only special `detail` value

Because the standalone remote MCP server is itself a tool provider, it does not need to reproduce Codex's internal model-capability gating exactly. In v1, the server should always expose `view_image`; the consuming client or agent is responsible for only calling it when it can consume image content.

## Public Session Identity

The public field name stays `session_id`, but its type should be `string`, not `number`.

Reasoning:

- the session must be broker-owned rather than target-local
- the ID must be opaque to callers
- multiple targets make numeric local process IDs misleading
- string IDs make collisions, guessability, and accidental target leakage less likely

Recommended format:

- unguessable random string
- URL-safe ASCII
- short prefix such as `sess_` is acceptable

Do not expose daemon-local process IDs or daemon session IDs to the caller.

## Broker Session Store

The broker stores one record per live public session:

- `session_id`
- `target`
- `daemon_session_id`
- `owner_identity`
- `created_at`
- `last_used_at`
- `daemon_instance_id`

`owner_identity` means whatever client identity the broker can practically observe in deployment. If the MCP deployment does not provide meaningful client identities, this field can degrade to broker-global ownership, but the internal model should still keep the field so stricter ownership can be added later without redesigning session routing.

`daemon_instance_id` is important for restart detection. Each daemon should expose a stable identity for its current process lifetime. If that value changes, the broker must treat all sessions mapped to the old daemon instance as invalid.

## Broker-To-Daemon Protocol

The internal broker-daemon protocol should not be MCP.

Use a separate internal RPC API with JSON request and response bodies over TLS. HTTP/2 is a good default transport. WebSocket over TLS is also acceptable if it simplifies deployment. v1 only needs unary request-response calls.

Recommended RPCs:

- `HealthCheck`
- `TargetInfo`
- `ExecStart`
- `ExecWrite`
- `PatchApply`
- `ImageRead`
- `SessionClose` optional in v1
- `SessionList` optional in v1

The protocol should include:

- `request_id`
- daemon version
- daemon instance ID
- structured error codes

### `TargetInfo`

Returns machine metadata used by the broker for validation and observability:

- daemon version
- daemon instance ID
- hostname
- OS and architecture
- capability flags such as PTY support and image-processing support

### `ExecStart`

Input:

- `cmd`
- `workdir`
- `shell`
- `tty`
- `yield_time_ms`
- `max_output_tokens`
- `login`

Response:

- `daemon_session_id?`
- `running: bool`
- `chunk_id?`
- `wall_time_seconds`
- `exit_code?`
- `original_token_count?`
- `output`
- optional canonicalized command metadata for logging

Behavior:

- if the command exits during the yield window, return `running=false` and no `daemon_session_id`
- if the command is still alive, return `running=true` and a `daemon_session_id`

### `ExecWrite`

Input:

- `daemon_session_id`
- `chars`
- `yield_time_ms`
- `max_output_tokens`

Response:

- `running: bool`
- `chunk_id?`
- `wall_time_seconds`
- `exit_code?`
- `original_token_count?`
- `output`

Behavior:

- empty `chars` acts as a poll
- non-empty `chars` writes stdin before polling
- daemon enforces the TTY requirement

### `PatchApply`

Input:

- `patch`
- `workdir`

Response:

- `output`
- optional semantic summary for observability

### `ImageRead`

Input:

- `path`
- `workdir`
- `detail`

Response:

- `image_url`
- `detail`

## Data Flow By Tool

### `exec_command`

1. Agent calls broker tool `exec_command`.
2. Broker validates arguments and resolves `target`.
3. Broker calls daemon `ExecStart`.
4. Daemon launches the local command using the documented local semantics.
5. If the command finished, broker returns a completed result.
6. If the command is still running, broker allocates a new public `session_id`, stores the session mapping, and returns the running result.

### `write_stdin`

1. Agent calls broker tool `write_stdin`.
2. Broker resolves `session_id` to a session record.
3. Broker optionally validates the supplied `target`.
4. Broker calls daemon `ExecWrite`.
5. If the daemon says the process is still running, broker returns the same public `session_id`.
6. If the daemon says the process exited, broker removes the session record and returns a completed result without `session_id`.

### `apply_patch`

1. Agent calls broker tool `apply_patch`.
2. Broker validates `target` and forwards the patch text and workdir to `PatchApply`.
3. Daemon parses, verifies, and applies the patch locally.
4. Broker returns the daemon's summary text as tool output.

### `view_image`

1. Agent calls broker tool `view_image`.
2. Broker validates `target` and forwards the path, workdir, and detail to `ImageRead`.
3. Daemon resolves and reads the image locally.
4. Daemon returns a data URL.
5. Broker wraps that into MCP image content plus structured content.

## Failure Model

Prefer predictable failure over best-effort recovery.

### Unknown target

If `target` is not configured, fail at the broker before contacting any daemon.

### Daemon unavailable

If the target daemon cannot be reached, fail the tool call with a target-unavailable error.

### Broker restart

Live session state does not survive broker restart in v1.

After restart:

- any prior `session_id` is unknown
- callers must rerun `exec_command`

### Daemon restart or session loss

Live session state does not survive daemon restart in v1.

The broker should detect daemon restart through `daemon_instance_id` changes or explicit session-not-found replies. When that happens:

- the affected broker session records are invalidated
- later `write_stdin` calls fail cleanly

### Network partition during a live session

Do not pretend the session is still healthy unless the daemon positively confirms it.

If broker-to-daemon communication fails for a live session:

- keep the session in a suspect state only long enough to attempt one clean revalidation
- if revalidation fails, invalidate the public session
- return a clear error rather than hanging or silently inventing output

### Command exit versus tool failure

Keep the existing distinction:

- a command that exits nonzero is a normal tool result with `exit_code`
- launch failures, routing failures, validation failures, and daemon RPC failures are tool errors

## Observability

Both broker and daemon should log enough to debug routing and session ownership without defaulting to full sensitive payload capture.

Broker logs should include:

- `request_id`
- tool name
- target
- public `session_id` when present
- daemon session ID when present
- daemon instance ID
- wall time
- result class such as completed, running, failed, invalidated

Daemon logs should include:

- `request_id`
- RPC name
- daemon session ID
- local PID when relevant
- wall time
- result class

By default, do not log:

- full shell command text
- full patch bodies
- full image data URLs
- stdin payloads

Allow these to be enabled only in explicit debug modes.

## Compatibility Notes By Tool

### `exec_command` and `write_stdin`

The daemon should stay as close as practical to the documented local behavior, including:

- yield clamps
- `write_stdin(chars="")` polling
- TTY restrictions
- output truncation
- transcript retention

The public remote result should stay recognizable to agents that already know the local tools, but the structured session ID type changes from numeric to opaque string.

### `apply_patch`

What should remain compatible:

- patch grammar
- verification rules
- file overwrite behavior
- non-atomic failure behavior
- success and failure text style

What can be intentionally cleaner:

- public API uses JSON input with `input`
- no shell-wrapper interception or approval pipeline in the broker

### `view_image`

What should remain compatible:

- data URL output
- resize behavior
- `"original"` handling
- path validation

What changes:

- the standalone server always exposes the tool
- capability gating moves to the consuming client or agent

## Testing Strategy

### Daemon tests

Build daemon tests directly from the behavioral rules in [local-system-tools.md](../local-system-tools.md).

Minimum daemon coverage:

- `exec_command` completion versus background-session behavior
- `write_stdin` polling behavior
- non-TTY stdin rejection
- session removal after exit
- patch grammar acceptance and rejection
- patch overwrite and move semantics
- non-atomic patch failure behavior
- image resize and original-detail behavior

### Broker tests

Minimum broker coverage:

- target validation
- public-to-daemon session mapping
- opaque public session ID allocation
- `write_stdin` routing to the owning daemon only
- invalidation after daemon restart
- invalidation after broker restart
- error translation for target-unavailable and session-not-found cases

### End-to-end tests

Run at least two daemons in test to prove multi-machine behavior:

- session IDs from target A cannot be used on target B
- concurrent live sessions on different targets remain isolated
- daemon restart invalidates only that daemon's sessions
- patch and image operations occur on the selected target only

## Rollout Recommendation

Implement in this order:

1. Build the Linux daemon with behavior cloned from the local-tool spec.
2. Build the broker with static target config and the four public tools.
3. Add broker session routing and restart invalidation.
4. Add observability and multi-daemon end-to-end coverage.
5. Add optional enhancements such as session listing or server-streaming exec deltas only after the base tool contract is stable.

## Summary

The cleanest v1 design is:

- public MCP broker
- one daemon per Linux target machine
- same four public tool names
- explicit `target` argument on machine-local tools
- broker-owned opaque `session_id`
- daemon-owned local process sessions
- no approval or sandbox complexity
- local behavior preserved at the daemon boundary using [local-system-tools.md](../local-system-tools.md) as the semantic reference

That gives a remote-first design that remains close to the existing Codex tool interface while removing the parts that only make sense for single-machine local execution.
