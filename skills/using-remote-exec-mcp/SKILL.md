---
name: using-remote-exec-mcp
description: Use when work must happen through a remote-exec-mcp broker on a named target or broker-host `local`, including target discovery, remote command execution, live session input, remote patching, optional hidden text-file tools, image reads, file transfer, port forwarding, or the `remote-exec` CLI
---

# Using remote-exec-mcp

This is an operator skill for using a configured `remote-exec-mcp` broker. It
does not require repository knowledge.

## Mental Model

- The broker exposes seven standard MCP tools: `remote_list_targets`,
  `remote_exec_command`, `remote_write_stdin`, `remote_apply_patch`,
  `remote_view_image`, `remote_transfer_files`, and `remote_forward_ports`.
- Brokers may additionally expose default-hidden `remote_read`, `remote_write`, and `remote_edit`
  tools only when config explicitly enables them.
- A broker can set `prepend_tool_names = false` for legacy unprefixed MCP tool
  names. Use the names advertised by the broker when that compatibility mode is enabled.
- Every machine-local operation is scoped to a logical `target`.
- `local` means the broker host, not necessarily your current shell.
- `session_id` and `forward_id` are opaque broker runtime tokens. Do not treat
  them as process IDs or daemon-local state.
- `remote_list_targets` is broker inventory backed by cached metadata. It performs a
  bounded recheck for unavailable or unhealthy remote targets before returning.
- `targets[].health_status` is `unknown`, `healthy`, `maybe_unhealthy`, or
  `unhealthy`. One failed probe changes `healthy` to `maybe_unhealthy`; the next
  failed probe changes it to `unhealthy`, while any successful probe restores
  `healthy`.
- A configured target can have `healthy: false` and `daemon_info: null`; stale
  daemon metadata is hidden while the target remains unhealthy.
- Connectivity may be direct or daemon-initiated reverse mode. This is
  transparent to MCP callers; reverse-lane loss surfaces as ordinary target
  unavailability or transport failure.
- Timed-out target operations return an error and are not replayed. After two
  consecutive timeouts without a successful response, the broker resets the
  target transport; one isolated timeout does not force a connection reset.
- Choosing a target grants broad access on that machine unless static sandbox
  config narrows the relevant path operation.
- A single command runs on one endpoint. Use `remote_transfer_files` to move bytes
  between endpoints.
- A port forward has a `listen_side` and a `connect_side`; swap them to reverse
  direction.
- `remote_transfer_files` can use `target: "local"` even when `local` does not appear
  in `remote_list_targets`.
- `remote_forward_ports` can use side `"local"` even when `local` does not appear in
  `remote_list_targets`.
- If broker structured content is disabled, rely on normal text/image content.
  `remote_apply_patch`, `remote_read`, `remote_write`, and `remote_edit` are text-only either way.
- Tool errors include `request_id`, `tool`, and `target` when known. Keep the
  request ID for broker and daemon log correlation.

## First Moves

1. Call `list_targets({})` unless the target name is already known.
2. Pick the target from `targets[].name`; do not guess names.
3. Use endpoint-native paths: `/srv/app/file` on Unix, `C:/work/file` on
   Windows. Windows targets may also accept MSYS/Cygwin-style paths such as
   `/c/work/file`.
4. Use `remote_exec_command`, `remote_apply_patch`, `remote_view_image`, or enabled hidden file
   tools for one endpoint.
5. Use `remote_transfer_files` for endpoint-to-endpoint copy.
6. Use `remote_forward_ports` for TCP/UDP tunneling.
7. If `remote_exec_command` returns `session_id`, keep it and use `remote_write_stdin` until
   the returned `session_id` becomes `null`.

## Tool Selection

- Discover targets, PTY support, and forwarding support: `remote_list_targets`
- Run a command on one target: `remote_exec_command`
- Continue or poll a live command: `remote_write_stdin`
- Edit files on one target with patch syntax: `remote_apply_patch`
- If enabled, read a text file with line prefixes: `remote_read`
- If enabled, overwrite or create a text file: `remote_write`
- If enabled, replace text in a file: `remote_edit`
- Read an image file from one target: `remote_view_image`
- Copy files or directories between endpoints: `remote_transfer_files`
- Open, list, or close TCP/UDP forwards: `remote_forward_ports`

## MCP JSON vs CLI Arguments

The MCP tools and the `remote-exec` CLI call the same broker behavior but do
not use the same input syntax.

- When calling MCP tools directly, use the JSON object shapes shown under
  **MCP Tools**. NEVER pass CLI shorthand strings in MCP tool calls.
- When using the `remote-exec` CLI, use the flag syntax shown under
  **`remote-exec` CLI**. The CLI accepts convenience shorthands and converts
  them into MCP-shaped requests.
- `remote_transfer_files` MCP endpoints are objects such as
  `{"target": "xp", "path": "C:/WINDOWS/win.ini"}`. NEVER send
  `xp:C:/WINDOWS/win.ini` or any other `target:path` CLI shorthand in MCP JSON;
  that shorthand is only for `remote-exec transfer-files`.
- `remote_forward_ports` MCP specs are objects with `listen_endpoint`,
  `connect_endpoint`, and `protocol`. NEVER send
  `tcp:127.0.0.1:15432=127.0.0.1:5432` or any other forward CLI shorthand in
  MCP JSON; that shorthand is only for `remote-exec forward-ports`.

## MCP Tools

### `remote_list_targets`

Input:

```json
{}
```

Use `targets[].healthy` for backward-compatible availability checks and
`targets[].health_status` to distinguish `healthy`, `maybe_unhealthy`,
`unhealthy`, and not-yet-checked `unknown` targets. A previously verified target
remains available during `maybe_unhealthy` while the broker schedules the next
probe using the shorter unhealthy interval. Use `daemon_info.platform` for path
choices, `supports_pty` before `tty: true`, and `supports_port_forward` before
remote forwarding.
`supports_port_forward` is true only when the target reports forwarding support
and the broker verifies a supported tunnel protocol version.

### `remote_exec_command`

Input:

```json
{
  "target": "builder-a",
  "cmd": "rg -n \"TODO|FIXME\" src",
  "workdir": "/srv/project",
  "tty": false,
  "yield_time_ms": 1000,
  "max_output_tokens": 4000
}
```

Guidance:

- Set `workdir` intentionally.
- Use `tty: true` for interactive programs or when later stdin input matters.
- Keep `session_id` when present.
- `session_id: null` means the command completed.
- `max_output_tokens` is approximate; output may be head/tail truncated.
- Read `warnings` when present.
- Do not send patch text through shell commands; use `remote_apply_patch`.

Optional fields: `workdir`, `shell`, `tty`, `yield_time_ms`,
`max_output_tokens`, `login`.

### `remote_write_stdin`

Input:

```json
{
  "session_id": "sess_...",
  "chars": "help\n",
  "yield_time_ms": 250,
  "max_output_tokens": 4000,
  "pty_size": {
    "rows": 33,
    "cols": 101
  },
  "target": "builder-a"
}
```

Guidance:

- Use only with a live `session_id`.
- Omit `chars` or send `chars: ""` to poll.
- Include `pty_size` for live TTY sessions when you need to resize before
  polling or writing. Omit `chars` for a resize-only poll. Do not use it for
  non-TTY sessions.
- `target` is optional, but if supplied it must match the original session.
- Reuse the returned `session_id` until it is `null`.
- Unknown or daemon-lost sessions surface as `Unknown process id ...`.
- If stdin was closed, rerun with `exec_command(..., "tty": true)`.

### `remote_apply_patch`

Input:

```json
{
  "target": "builder-a",
  "workdir": "/srv/project",
  "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-old\n+new\n*** End Patch\n"
}
```

Guidance:

- Use normal Codex patch syntax.
- Relative patch paths resolve from `workdir` when supplied.
- Existing `LF` versus `CRLF` style is preserved for updated files.
- Empty patch envelopes, blank update context lines, blank separators after
  `*** End of File`, and Unicode whitespace around patch control lines are accepted.
- `remote_apply_patch` preflights deterministic failures before writing, including
  missing files, non-file targets, sandbox denial, decode failures, and
  unmatched hunks.
- Multi-file patches are still not transactional for runtime races or
  write/remove failures after preflight. If that residual partial-application
  risk would be hard to recover from, split the patch.
- Successful calls return text output only.

### Hidden `remote_read`, `remote_write`, `remote_edit`

These tools are default-hidden and may be absent from `list_tools`.

Read:

```json
{
  "target": "builder-a",
  "file_path": "src/main.rs",
  "offset": 1,
  "limit": 2000
}
```

Write:

```json
{
  "target": "builder-a",
  "file_path": "notes.txt",
  "content": "hello\n"
}
```

Edit:

```json
{
  "target": "builder-a",
  "file_path": "notes.txt",
  "old_string": "hello",
  "new_string": "hello world",
  "replace_all": false
}
```

Guidance:

- No `workdir` field exists. Relative paths resolve from the target
  daemon/default workdir.
- `read.offset` is one-based; omitted or `0` means line `1`.
- `read.limit` is in lines and defaults to the broker config limit.
- `remote_read` prefixes every returned line as `N: text` and ends with a reminder
  describing EOF, an out-of-range offset, an empty file, or the displayed range.
- `remote_write` overwrites the file or creates it if missing.
- `remote_edit` rejects multiple `old_string` matches unless `replace_all` is true.
- `remote_read`, `remote_write`, and `remote_edit` use the same experimental target encoding
  autodetection policy as `remote_apply_patch` when enabled.
- If these tools are absent, use `remote_exec_command`, `remote_apply_patch`, or
  `remote_transfer_files` instead.

### `remote_view_image`

Input:

```json
{
  "target": "builder-a",
  "path": "/srv/project/chart.png",
  "detail": "original"
}
```

Guidance:

- Use `workdir` only for relative path resolution.
- `detail` is accepted for compatibility but has no effect.
- PNG, JPEG, and WebP are returned without resizing.
- Rust daemon also transcodes BMP, GIF, ICO, PNM (including PPM), and TGA to
  PNG; C++ daemon targets support PNG, JPEG, and WebP only.

### `remote_transfer_files`

Input:

```json
{
  "sources": [
    {"target": "local", "path": "/tmp/report.txt"},
    {"target": "local", "path": "/tmp/screenshots"}
  ],
  "destination": {"target": "builder-a", "path": "/srv/inbox"},
  "exclude": ["**/*.log", ".git/**"],
  "overwrite": "merge",
  "destination_mode": "auto",
  "symlink_mode": "preserve",
  "create_parent": true
}
```

Required:

- exactly one of `source` or `sources`
- `destination.target`
- `destination.path`
- `create_parent`

Guidance:

- MCP `source`, `sources[]`, and `destination` are endpoint objects. NEVER send
  CLI shorthand strings like `"local:/tmp/source.txt"` or
  `"builder-a:/tmp/dest.txt"` as MCP endpoint values.
- CLI equivalent:
  `remote-exec transfer-files --source local:/tmp/source.txt --destination builder-a:/tmp/dest.txt`.
- Paths must be absolute for their own endpoint.
- `destination_mode: "auto"` gives single-source transfers `cp`-like behavior:
  copy under `destination.path` if it is an existing directory or ends in a path
  separator, otherwise use it as the exact final path. Multi-source transfers
  treat `destination.path` as a directory root.
- Use `destination_mode: "into_directory"` to always place sources under the
  destination by basename.
- Use `destination_mode: "exact"` to force exact final-path behavior.
- `overwrite: "merge"` overlays without deleting unrelated directory entries.
- `overwrite: "replace"` replaces a single file or directory destination. For
  multi-source directory transfers, it replaces only incoming top-level
  destination entries and preserves unrelated existing entries.
- Transfers are not transactional; a failure can leave partial destination
  changes.
- `exclude` is matched relative to each source root with `/` as the logical
  separator on every platform.
- `symlink_mode` is `preserve`, `follow`, or `skip`.
- Do not send a public `compression` field; compression is broker-internal.
- Prefer `remote_transfer_files` over `scp`, shell redirection, or ad hoc archives for
  cross-endpoint data movement.

### `remote_forward_ports`

Open:

```json
{
  "action": "open",
  "listen_side": "local",
  "connect_side": "builder-a",
  "forwards": [
    {
      "listen_endpoint": "127.0.0.1:15432",
      "connect_endpoint": "127.0.0.1:5432",
      "protocol": "tcp"
    }
  ]
}
```

List:

```json
{"action": "list", "forward_ids": ["fwd_..."]}
```

Close:

```json
{"action": "close", "forward_ids": ["fwd_..."]}
```

Guidance:

- MCP `forwards[]` entries are objects. Do not send CLI shorthand strings like
  `"tcp:127.0.0.1:15432=127.0.0.1:5432"` in MCP tool calls.
- CLI equivalent:
  `remote-exec forward-ports open --forward tcp:127.0.0.1:15432=127.0.0.1:5432`.
- Supported protocols are `tcp` and `udp`.
- Bare endpoint strings like `"8080"` mean `"127.0.0.1:8080"`.
- `listen_endpoint` may use port `0`; read the returned `listen_endpoint` for
  the actual bound port.
- `connect_endpoint` must use a nonzero port.
- Non-loopback listen binds such as `"0.0.0.0:8080"` are allowed.
- Keep `forward_id` and close it explicitly when done.
- Human-readable tool text groups forwards by ready/not-ready state and shows
  the exposed endpoints; inspect structured content for detailed phase, health,
  reconnect, and accounting fields.
- Treat a forward as ready only when `phase = "ready"`. Legacy
  `status = "open"` can coexist with `phase = "reconnecting"`.
- If a forward leaves `ready`, inspect `phase` and reopen it when needed.
- Broker or target restart destroys useful public forward state; open a new
  forward.

## `remote-exec` CLI

The CLI calls the same public broker tools.

Connection modes:

```bash
remote-exec --broker-config configs/broker.example.toml list-targets
remote-exec --broker-url http://127.0.0.1:8787/mcp list-targets
```

- `--broker-config PATH` loads broker config and invokes handlers in-process.
  It does not start a long-running MCP broker.
- `--broker-url URL` connects to a running streamable-HTTP broker.
- Use `--json` to print the normalized tool response object.
- Exit codes: `0` success, `2` usage/input, `3` broker config load/build, `4`
  streamable-HTTP connection/transport, `5` MCP tool error.
- For persistent `forward-ports`, prefer `--broker-url`; in `--broker-config`
  mode each CLI invocation has fresh broker memory.

Common CLI examples:

```bash
remote-exec --broker-config configs/broker.example.toml \
  exec --target builder-a --workdir /srv/project 'cargo test'

remote-exec --broker-config configs/broker.example.toml \
  write-stdin --session-id sess_... --chars $'q\n'

remote-exec --broker-config configs/broker.example.toml \
  apply-patch --target builder-a --workdir /srv/project --input-file -

remote-exec --broker-config configs/broker.example.toml \
  view-image --target builder-a --path /srv/project/chart.png --out ./chart.png

remote-exec --broker-config configs/broker.example.toml \
  transfer-files \
  --source local:/tmp/source.txt \
  --destination builder-a:/tmp/dest.txt \
  --overwrite replace \
  --create-parent

remote-exec --broker-url http://127.0.0.1:8787/mcp \
  forward-ports open \
  --listen-side local \
  --connect-side builder-a \
  --forward tcp:127.0.0.1:15432=127.0.0.1:5432
```

CLI names use kebab-case (`list-targets`, `write-stdin`, `transfer-files`).
The command runner accepts `exec` as an alias for `exec-command`.

Use `--input-file -` for patch stdin and `--chars-file -` for session input
stdin.

## Standard Workflows

Inspect and edit remote code:

1. `remote_list_targets`
2. `remote_exec_command` to inspect/search
3. `remote_apply_patch` on that target
4. `remote_exec_command` to verify

Upload, run, retrieve:

1. `remote_transfer_files` from `local` to target
2. `remote_exec_command` on target
3. `remote_transfer_files` from target to `local` for artifacts

Interactive session:

1. Check `supports_pty`
2. `remote_exec_command` with `tty: true`
3. Use `remote_write_stdin` to send input or poll

Port forward:

1. `remote_list_targets` and confirm forwarding support
2. `remote_forward_ports` open
3. `remote_forward_ports` list until `phase = "ready"`
4. Use the forwarded service
5. `remote_forward_ports` close

## Capability Notes

- If you can connect to a target through the broker, treat it as supported for
  the tools and capabilities it reports.
- Do not infer behavior from operating system names, daemon implementation,
  version strings, or build labels.
- Trust `remote_list_targets` health and capability fields and the tools exposed by the
  broker.
- Use `supports_pty` before `tty: true`; if PTY support is false or unknown,
  run non-interactively or expect a typed unsupported error.
- Use `supports_port_forward` before opening forwards.
- Optional tools, image detail modes, transfer features, shell behavior, and
  stdin behavior can vary by target. Read tool results, warnings, and errors.

## Common Mistakes

- Guessing target names instead of calling `remote_list_targets`.
- Forgetting that `local` means broker host.
- Running a command on one target and expecting it to read another target's
  filesystem.
- Using shell tricks instead of `remote_transfer_files` for cross-endpoint copy.
- Copying CLI endpoint or forward shorthand into direct MCP tool calls.
- Sending relative paths to `remote_transfer_files`.
- Assuming `overwrite: "merge"` deletes destination files absent from source.
- Treating `status = "open"` as readiness for `remote_forward_ports`; check `phase`.
- Leaving port forwards open after use.
- Reusing `session_id` or `forward_id` after broker restart.
- Sending patch text through `remote_exec_command` instead of `remote_apply_patch`.
