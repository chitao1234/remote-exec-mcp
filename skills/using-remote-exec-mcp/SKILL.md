---
name: using-remote-exec-mcp
description: Use when work must happen through a remote-exec-mcp broker on a named target or broker-host `local`, including target discovery, remote command execution, live session input, remote patching, image reads, file transfer, port forwarding, or the `remote-exec` CLI
---

# Using remote-exec-mcp

This is an operator skill for using a configured `remote-exec-mcp` broker. It
does not require repository knowledge.

## Mental Model

- The broker exposes seven MCP tools: `list_targets`, `exec_command`,
  `write_stdin`, `apply_patch`, `view_image`, `transfer_files`, and
  `forward_ports`.
- Every machine-local operation is scoped to a logical `target`.
- `local` means the broker host, not necessarily your current shell.
- `session_id` and `forward_id` are opaque broker runtime tokens. Do not treat
  them as process IDs or daemon-local state.
- `list_targets` is cached inventory, not a live health probe. A configured
  target can have `daemon_info: null`.
- Choosing a target grants broad access on that machine unless static sandbox
  config narrows the relevant path operation.
- A single command runs on one endpoint. Use `transfer_files` to move bytes
  between endpoints.
- A port forward has a `listen_side` and a `connect_side`; swap them to reverse
  direction.
- `transfer_files` can use `target: "local"` even when `local` does not appear
  in `list_targets`.
- `forward_ports` can use side `"local"` even when `local` does not appear in
  `list_targets`.
- If broker structured content is disabled, rely on normal text/image content.
  `apply_patch` is text-only either way.
- Tool errors include `request_id`, `tool`, and `target` when known. Keep the
  request ID for broker and daemon log correlation.

## First Moves

1. Call `list_targets({})` unless the target name is already known.
2. Pick the target from `targets[].name`; do not guess names.
3. Use endpoint-native paths: `/srv/app/file` on Unix, `C:/work/file` on
   Windows. Windows targets may also accept MSYS/Cygwin-style paths such as
   `/c/work/file`.
4. Use `exec_command`, `apply_patch`, or `view_image` for one endpoint.
5. Use `transfer_files` for endpoint-to-endpoint copy.
6. Use `forward_ports` for TCP/UDP tunneling.
7. If `exec_command` returns `session_id`, keep it and use `write_stdin` until
   the returned `session_id` becomes `null`.

## Tool Selection

- Discover targets, PTY support, and forwarding support: `list_targets`
- Run a command on one target: `exec_command`
- Continue or poll a live command: `write_stdin`
- Edit files on one target with patch syntax: `apply_patch`
- Read an image file from one target: `view_image`
- Copy files or directories between endpoints: `transfer_files`
- Open, list, or close TCP/UDP forwards: `forward_ports`

## MCP Tools

### `list_targets`

Input:

```json
{}
```

Use `daemon_info.platform` for path choices, `supports_pty` before `tty: true`,
and `supports_port_forward` / `port_forward_protocol_version` before remote
forwarding. `port_forward_protocol_version: 4` means the target uses v4 tunnel
semantics.

### `exec_command`

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
- Do not send patch text through shell commands; use `apply_patch`.

Optional fields: `workdir`, `shell`, `tty`, `yield_time_ms`,
`max_output_tokens`, `login`.

### `write_stdin`

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

### `apply_patch`

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
- Multi-file patches are not transactional. If partial application would be
  hard to recover from, split the patch.
- Successful calls return text output only.

### `view_image`

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
- Omit `detail` for the target default.
- Use `detail: "original"` for full-fidelity inspection.
- C++ daemon targets support passthrough PNG, JPEG, and WebP only, and omitted
  `detail` defaults to `original`.

### `transfer_files`

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

- Paths must be absolute for their own endpoint.
- `destination_mode: "auto"` gives single-source transfers `cp`-like behavior:
  copy under `destination.path` if it is an existing directory or ends in a path
  separator, otherwise use it as the exact final path. Multi-source transfers
  treat `destination.path` as a directory root.
- Use `destination_mode: "into_directory"` to always place sources under the
  destination by basename.
- Use `destination_mode: "exact"` to force exact final-path behavior.
- `overwrite: "merge"` overlays without deleting unrelated directory entries.
- `overwrite: "replace"` removes the destination first.
- `exclude` is matched relative to each source root with `/` as the logical
  separator on every platform.
- `symlink_mode` is `preserve`, `follow`, or `skip`.
- Do not send a public `compression` field; compression is broker-internal.
- Prefer `transfer_files` over `scp`, shell redirection, or ad hoc archives for
  cross-endpoint data movement.

### `forward_ports`

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

- Supported protocols are `tcp` and `udp`.
- Bare endpoint strings like `"8080"` mean `"127.0.0.1:8080"`.
- `listen_endpoint` may use port `0`; read the returned `listen_endpoint` for
  the actual bound port.
- `connect_endpoint` must use a nonzero port.
- Non-loopback listen binds such as `"0.0.0.0:8080"` are allowed.
- Keep `forward_id` and close it explicitly when done.
- Treat a forward as ready only when `phase = "ready"`. Legacy
  `status = "open"` can coexist with `phase = "reconnecting"`.
- v4 forwards may recover from broker-daemon transport loss if the daemon stays
  alive. Active TCP streams and UDP per-peer connector state do not survive
  reconnect.
- Broker or daemon restart destroys the useful public forward state; open a new
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

1. `list_targets`
2. `exec_command` to inspect/search
3. `apply_patch` on that target
4. `exec_command` to verify

Upload, run, retrieve:

1. `transfer_files` from `local` to target
2. `exec_command` on target
3. `transfer_files` from target to `local` for artifacts

Interactive session:

1. Check `supports_pty`
2. `exec_command` with `tty: true`
3. Use `write_stdin` to send input or poll

Port forward:

1. `list_targets` and confirm forwarding support
2. `forward_ports` open
3. `forward_ports` list until `phase = "ready"`
4. Use the forwarded service
5. `forward_ports` close

## Compatibility Notes

- PTY support is target-specific. Trust `supports_pty`.
- Rust daemons support TLS by default; C++ daemon targets are plain HTTP.
- C++ daemon targets are narrower than Rust daemon targets.
- The C++ daemon uses C++11 on every supported build path. "Windows
  XP-compatible" means the binary was built with a toolchain that supports both
  XP targeting and C++11.
- POSIX C++ daemon targets support `tty: true` when PTY allocation is available.
- Windows XP-compatible C++ targets reject `tty: true` and use `cmd.exe`.
- POSIX C++ non-TTY exec starts with stdin closed. Use `tty: true` when later
  input matters.
- Windows XP-compatible C++ non-TTY exec keeps stdin open for compatibility.
- C++ transfers support regular files, directory trees, and broker-built
  multi-source bundles. Compression, hard links, sparse files, and special files
  are not public features there.
- C++ forwarding uses the v4 tunnel protocol and reports the same public
  `forward_ports` state shape as Rust targets where supported.

## Common Mistakes

- Guessing target names instead of calling `list_targets`.
- Forgetting that `local` means broker host.
- Running a command on one target and expecting it to read another target's
  filesystem.
- Using shell tricks instead of `transfer_files` for cross-endpoint copy.
- Sending relative paths to `transfer_files`.
- Assuming `overwrite: "merge"` deletes destination files absent from source.
- Treating `status = "open"` as readiness for `forward_ports`; check `phase`.
- Leaving port forwards open after use.
- Reusing `session_id` or `forward_id` after broker restart.
- Sending patch text through `exec_command` instead of `apply_patch`.
