# remote-exec-daemon-cpp

Standalone C++ daemon for `remote-exec-mcp`.

This daemon is intentionally narrower than the Rust daemon, but it now has two
build paths:

- native POSIX hosts through `g++`
- Windows XP-compatible hosts through `i686-w64-mingw32-g++`

The former `remote-exec-daemon-xp` name referred to the original Windows XP-only
shape. Current live behavior is documented here and in the repository root
`README.md`. The dated material under the top-level `docs/` tree is historical
implementation detail, not the current contract.

## Build

Build outputs are written to this directory's `build/` tree even when `make` is
invoked from another working directory.

POSIX daemon:

- `make all-posix`
- `make check-posix`

Windows XP-compatible daemon:

- `make all-windows-xp`
- `make test-wine-session-store` when `wine` is available

Default host-native verification:

- `make check`

Focused host-native tests:

- `make test-host-patch`
- `make test-host-transfer`
- `make test-host-config`
- `make test-host-http-request`
- `make test-host-session-store`
- `make test-host-server-routes`

## Run

POSIX:

```sh
build/remote-exec-daemon-cpp config/daemon-cpp.example.ini
```

Windows XP-compatible:

```bat
build\remote-exec-daemon-cpp-xp.exe config\daemon-cpp.example.ini
```

Logs go to `stderr`. Set `REMOTE_EXEC_LOG=debug` to raise the level, or use a
shared filter string such as
`REMOTE_EXEC_LOG=warn,remote_exec_daemon_cpp=debug`.

Non-TTY exec output merges `stdout` and `stderr` through one pipe, so the
returned `output` field preserves their emitted order.

POSIX builds support `tty=true` when the host can allocate a PTY. The daemon
reports this through `/v1/target-info` as `supports_pty`, and rejects `tty=true`
only when PTY allocation is unavailable. Windows XP-compatible builds always
report `supports_pty=false`.

POSIX non-TTY exec intentionally starts child processes with stdin attached to
`/dev/null`, matching the Rust daemon's closed-stdin behavior. Start POSIX
interactive commands with `tty=true` when later `write_stdin` input is needed.
Windows XP-compatible non-TTY exec intentionally keeps its pipe-backed stdin
open to preserve the original XP daemon behavior.

The C++ daemon implements the daemon-side port RPCs used by broker
`forward_ports`: TCP listeners/connectors, UDP datagram sockets, non-loopback
listen binds, and the same bare-port normalization where `8080` means
`127.0.0.1:8080`.

## Config

Example config:

```ini
target = builder-cpp
listen_host = 0.0.0.0
listen_port = 8181

# POSIX example.
default_workdir = /work
# default_shell = /bin/bash

# Windows XP example.
# default_workdir = C:\work
# default_shell = cmd.exe

# Login shells default to enabled.
# allow_login_shell = true

# Optional HTTP bearer auth. This authenticates broker requests but does not
# add encryption or integrity protection on plain HTTP.
# http_auth_bearer_token = replace-me

# Request/session safety limits.
# max_request_header_bytes = 65536
# max_request_body_bytes = 536870912
# max_open_sessions = 64

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

## Shell Policy

- POSIX default shell selection follows the Rust daemon's policy: configured
  `default_shell`, then `SHELL`, passwd shell, `bash`, and `/bin/sh`.
- POSIX exec uses `shell -c <cmd>` or `shell -l -c <cmd>` for login shells.
- POSIX child processes currently force `LC_ALL=C.UTF-8` and `LANG=C.UTF-8`.
- Windows XP-compatible exec supports `cmd.exe`; `login=false` adds `/D` before
  `/C`, while `login=true` omits `/D`.

## Limitations

- plain HTTP only, with optional bearer-auth request authentication
- no TLS support
- no static sandbox support
- PTY support is POSIX-only and depends on host PTY allocation
- no PTY support in Windows XP-compatible builds
- no image support
- transfer compression is not supported
- `transfer_files` supports regular files, directory trees, and broker-built multi-source bundles
- transfer imports support `fail`, `merge`, and `replace` overwrite modes; `merge` overlays compatible existing destinations without deleting unrelated directory entries
- POSIX transfer exports skip unsupported special entries in directory trees and report warnings
- POSIX transfer symlink modes support preserving, following, skipping, or rejecting symlinks
- Windows XP-compatible transfer builds do not support preserving or following symlinks; use skip or reject modes
- transfer payloads use GNU tar for files and directories
- single-file transfers use the fixed archive entry `.remote-exec-file`
- transfer warnings use the reserved archive summary entry `.remote-exec-transfer-summary.json`, which is consumed during import and is not extracted
- unsupported archive entries remain rejected: hard links, special files unless skipped during export, sparse entries, and malformed paths
- broker targets that point at this daemon must use `http://...` plus `allow_insecure_http = true`
- optional `http_auth_bearer_token` can require `Authorization: Bearer ...` from the broker, but it still does not encrypt plain-HTTP traffic
