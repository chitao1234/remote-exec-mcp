# remote-exec-daemon-xp

Standalone Windows XP daemon for `remote-exec-mcp`.

Current live behavior is documented here and in the repository root `README.md`. The dated material under the top-level `docs/` tree is historical implementation detail, not the current XP contract.

## Build

`make all`

Host-native verification:

- `make test-host-patch`
- `make test-host-transfer`
- `make test-host-config`
- `make test-host-http-request`
- `make check`

Wine-backed XP exec verification, when `wine` is available:

- `make test-wine-session-store`

## Run

`build/remote-exec-daemon-xp.exe config/daemon-xp.example.ini`

Logs go to `stderr`. Set `REMOTE_EXEC_LOG=debug` to raise the level, or use a shared filter string such as `REMOTE_EXEC_LOG=warn,remote_exec_daemon_xp=debug`.

Non-TTY exec output merges `stdout` and `stderr` through one pipe, so the returned `output` field preserves their emitted order.

`exec_command` and `write_stdin` use the same truncation contract as the main daemon:

- `max_output_tokens` is approximate, with one token treated as about four UTF-8 bytes
- `original_token_count` is `ceil(total_utf8_bytes / 4)`
- omitted `max_output_tokens` still defaults to `10000`, while explicit `0` returns an empty `output`
- when truncation happens, `output` becomes `Total output lines: N\n\n{head}…X tokens truncated…{tail}` with a UTF-8-safe roughly 50/50 head/tail split

## Config

Example config:

```ini
target = builder-xp
listen_host = 0.0.0.0
listen_port = 8181
default_workdir = C:\work
# Optional HTTP bearer auth. This authenticates broker requests but does not
# add encryption or integrity protection on plain HTTP.
# http_auth_bearer_token = replace-me
# Optional per-operation yield-time policy overrides.
# yield_time_exec_command_default_ms = 10000
# yield_time_exec_command_max_ms = 30000
# yield_time_exec_command_min_ms = 250
# yield_time_write_stdin_poll_default_ms = 5000
# yield_time_write_stdin_poll_max_ms = 300000
# yield_time_write_stdin_poll_min_ms = 5000
# yield_time_write_stdin_input_default_ms = 250
# yield_time_write_stdin_input_max_ms = 30000
# yield_time_write_stdin_input_min_ms = 250
```

## Limitations

- plain HTTP only in v1, with optional bearer-auth request authentication
- no PTY support
- no image support
- `transfer_files` supports regular files, directory trees, and broker-built multi-source bundles
- transfer payloads use GNU tar for both files and directories
- single-file transfers use the fixed archive entry `.remote-exec-file`
- transfer compression is not supported; XP only accepts uncompressed payloads
- unsupported archive entries remain rejected: symlinks, hard links, special files, sparse entries, and malformed paths
- `cmd.exe` only for shell execution
- broker targets that point at XP must use `http://...` plus `allow_insecure_http = true`
- optional `http_auth_bearer_token` can require `Authorization: Bearer ...` from the broker, but it still does not encrypt plain-HTTP traffic
