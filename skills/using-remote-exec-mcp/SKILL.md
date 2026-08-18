---
name: using-remote-exec-mcp
description: Use remote-exec-mcp tools or the `remote-exec` CLI to discover available endpoints, run and continue commands, patch or edit files, view images, transfer files, and manage TCP or UDP port forwards. Use when work must happen through remote-exec-mcp on a named target or the logical `local` endpoint.
---

# Using remote-exec-mcp

Use the available remote-exec-mcp connection to work on named endpoints. Assume
the service and targets are already available; focus on completing the user's
task through the exposed tools.

## Start Here

1. Call `remote_list_targets({})` unless the target is already known.
2. Select an advertised target and check its health and capabilities.
3. Use paths native to that endpoint, such as `/srv/app/file` on Unix or
   `C:/work/file` on Windows.
4. Choose the narrowest tool that completes the operation.
5. Keep any returned `session_id` or `forward_id` only for the active command or
   forward.

Treat `local` as a logical endpoint. Do not assume it is the machine running the
current agent. Use it only where the exposed tools accept it.

## Choose a Tool

- Discover targets and capabilities: `remote_list_targets`
- Run a command: `remote_exec_command`
- Continue, poll, or resize a live command: `remote_write_stdin`
- Apply a patch: `remote_apply_patch`
- Read, write, or replace text when exposed: `remote_read`, `remote_write`,
  `remote_edit`
- Read an image: `remote_view_image`
- Copy files or directories between endpoints: `remote_transfer_files`
- Open, inspect, or close TCP or UDP forwards: `remote_forward_ports`

Use the exact tool names exposed in the current session.

## Keep MCP and CLI Syntax Separate

Pass JSON objects to MCP tools. Do not pass CLI shorthand as MCP input.

- Use `{"target": "builder-a", "path": "/srv/file"}` for an MCP transfer
  endpoint, not `builder-a:/srv/file`.
- Use objects with `listen_endpoint`, `connect_endpoint`, and `protocol` for MCP
  forwards, not `tcp:127.0.0.1:15432=127.0.0.1:5432`.
- Use `target:path` and compact forward specifications only with the
  `remote-exec` CLI.

## Use MCP Tools

### `remote_list_targets`

Call with an empty object:

```json
{}
```

Use `targets[].name` instead of guessing a target name. Prefer a target with
`healthy: true`. Use `health_status` when present to distinguish `healthy`,
`maybe_unhealthy`, `unhealthy`, and `unknown`.

Use the reported platform to choose path syntax. Check `supports_exec`,
`supports_apply_patch`, `supports_pty`, and `supports_port_forward` before
depending on those capabilities.

### `remote_exec_command`

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

- Set `workdir` intentionally when the command depends on repository-relative
  paths.
- Set `tty: true` for an interactive program or a command that needs later
  input.
- Treat `session_id: null` as completion. Otherwise, continue with
  `remote_write_stdin`.
- Check `exit_code`, `output`, and `warnings` before deciding that the command
  succeeded.
- Expect long output to be truncated when `max_output_tokens` is set.
- Use `remote_apply_patch` for patch text instead of sending it through a shell.

Optional fields are `workdir`, `shell`, `tty`, `yield_time_ms`,
`max_output_tokens`, and `login`.

### `remote_write_stdin`

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

- Use only a live `session_id` returned by `remote_exec_command` or the previous
  `remote_write_stdin` call.
- Omit `chars` or send an empty string to poll without writing.
- Use `pty_size` only for a TTY session.
- Omit `chars` to perform a resize-only poll.
- Omit `target`, or supply the target that created the session.
- Continue until the returned `session_id` is `null`.

### `remote_apply_patch`

```json
{
  "target": "builder-a",
  "workdir": "/srv/project",
  "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-old\n+new\n*** End Patch\n"
}
```

- Use standard Codex patch syntax.
- Set `workdir` when using relative patch paths.
- Treat a syntax error as no change, but treat a later file or hunk failure as
  potentially partial: earlier actions in the same patch may already have
  completed.
- Inspect the returned text for the affected paths or failure details.

### Optional `remote_read`, `remote_write`, and `remote_edit`

Use these tools only when they are exposed.

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

- Treat `offset` as one-based; use `0` or omit it to start at line 1.
- Treat `limit` as a line count.
- Expect `remote_read` to prefix returned lines with line numbers.
- Expect `remote_write` to create or overwrite the file.
- Set `replace_all: true` only when every match should change; otherwise,
  `remote_edit` rejects multiple matches.
- Use `remote_exec_command`, `remote_apply_patch`, or `remote_transfer_files`
  when these optional tools are absent.

### `remote_view_image`

```json
{
  "target": "builder-a",
  "path": "/srv/project/chart.png"
}
```

Set `workdir` when using a relative path. Use the returned image content for
inspection, and use `remote_exec_command` on the same target to convert an
unsupported format when necessary.

### `remote_transfer_files`

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

- Provide exactly one of `source` or `sources`.
- Use endpoint objects for every source and destination.
- Use absolute paths in each endpoint's own path syntax.
- Set `create_parent` explicitly.
- Use `destination_mode: "auto"` for normal copy behavior. For one source, copy
  under an existing directory or a path ending in a separator; otherwise, use
  the destination as the exact final path. For multiple sources, treat the
  destination as a directory.
- Use `destination_mode: "into_directory"` to always place each source under
  the destination by basename.
- Use `destination_mode: "exact"` to force an exact final path for one source.
- Use `overwrite: "merge"` to preserve unrelated destination entries.
- Use `overwrite: "replace"` to replace the incoming destination entry. For
  multiple sources, preserve unrelated top-level entries.
- Match `exclude` patterns relative to each source root with `/` separators.
- Choose `symlink_mode` from `preserve`, `follow`, or `skip`.
- Treat transfer failure as potentially partial and inspect the destination
  before retrying.

Prefer this tool over shell redirection, `scp`, or temporary archives for
cross-endpoint copies.

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

- Put the accepting endpoint on `listen_side` and the destination service on
  `connect_side`; swap them to reverse the direction.
- Choose `tcp` or `udp` for each forward.
- Use port `0` on `listen_endpoint` to request an available port, then read the
  returned endpoint for the selected port.
- Use a nonzero port on `connect_endpoint`.
- Bind to a non-loopback address only when the service should be exposed beyond
  the listening machine.
- Treat the forward as usable only when `phase` is `ready`.
- Keep each `forward_id`, inspect `last_error` when a forward is not ready, and
  close every forward when finished.

## Use the CLI

Use the CLI only when requested or when direct MCP tools are unavailable. Pass
the broker URL supplied for the current environment:

```bash
remote-exec --broker-url "$BROKER_URL" list-targets

remote-exec --broker-url "$BROKER_URL" \
  exec --target builder-a --workdir /srv/project 'cargo test'

remote-exec --broker-url "$BROKER_URL" \
  apply-patch --target builder-a --workdir /srv/project --input-file -

remote-exec --broker-url "$BROKER_URL" \
  transfer-files \
  --source local:/tmp/source.txt \
  --destination builder-a:/tmp/dest.txt \
  --overwrite replace \
  --create-parent

remote-exec --broker-url "$BROKER_URL" \
  forward-ports open \
  --listen-side local \
  --connect-side builder-a \
  --forward tcp:127.0.0.1:15432=127.0.0.1:5432
```

Use kebab-case CLI names such as `list-targets`, `write-stdin`, and
`transfer-files`. Use `exec` as the short form of `exec-command`. Use `--json`
for structured output, `--input-file -` for patch input from stdin, and
`--chars-file -` for session input from stdin. Run `remote-exec --help` or a
subcommand's `--help` for less common flags.

## Follow Common Workflows

Inspect and edit code:

1. Call `remote_list_targets`.
2. Inspect with `remote_exec_command` or `remote_read`.
3. Edit with `remote_apply_patch` or `remote_edit`.
4. Verify with `remote_exec_command`.

Upload, run, and retrieve:

1. Transfer input from `local` to the target.
2. Run the command on the target.
3. Transfer artifacts from the target to `local`.

Use an interactive session:

1. Confirm `supports_pty`.
2. Start `remote_exec_command` with `tty: true`.
3. Send input or poll with `remote_write_stdin` until completion.

Use a port forward:

1. Confirm `supports_port_forward`.
2. Open the forward.
3. Wait for `phase: "ready"`.
4. Use the forwarded service.
5. Close the forward.

## Avoid Common Mistakes

- Do not guess target names.
- Do not assume `local` refers to the current agent's machine.
- Do not expect a command on one target to read another target's filesystem.
- Do not copy CLI shorthand into MCP JSON.
- Do not send relative paths to `remote_transfer_files`.
- Do not expect `overwrite: "merge"` to delete unrelated destination entries.
- Do not treat `status: "open"` alone as forward readiness; check `phase`.
- Do not leave port forwards open after use.
- Do not reuse a session or forward ID after it stops resolving.
