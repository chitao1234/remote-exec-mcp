# remote-exec-daemon-cpp

Standalone C++ daemon for `remote-exec-mcp`.

This daemon is intentionally narrower than the Rust daemon, but it now has two
build paths:

- native POSIX hosts through `g++`
- Windows XP-compatible hosts through `i686-w64-mingw32-g++`
- Windows XP-compatible hosts through MSVC/NMAKE with the `v141_xp` toolset

The former `remote-exec-daemon-xp` name referred to the original Windows XP-only
shape. Current live behavior is documented here and in the repository root
`README.md`. The dated material under the top-level `docs/` tree is historical
implementation detail, not the current contract.

## Build

Build outputs are written to this directory's `build/` tree even when `make` is
invoked from another working directory. Incremental builds reuse cached object
files under `build/obj/`, so repeated `make` runs only rebuild sources whose
inputs changed.

Host-native POSIX daemon:

- `make`
- `make all-posix`
- `make check`
- `make check-posix`

Windows XP-compatible cross-build:

- `make all-windows-xp`
- `make check-windows-xp`
- `make test-wine-session-store` when `wine` is available

Windows XP-compatible MSVC/NMAKE build:

- Open an x86 Visual Studio developer prompt with the XP-capable VS 2017
  toolset, such as `vcvarsall.bat x86 -vcvars_ver=14.16`.
- `nmake /f NMakefile`
- `nmake /f NMakefile all-msvc-xp`
- `nmake /f NMakefile check-msvc-xp`

The top-level `GNUmakefile` is the GNU make public entry point. Shared source
lists live in `mk/sources.mk`, shared GNU make helpers live in `mk/common.mk`,
host-native rules live in `mk/posix.mk`, and Windows XP cross-build rules live
in `mk/windows-xp.mk`. GNU make prefers `GNUmakefile`, so plain `make` selects
that file automatically.

`NMakefile` is intentionally separate from the GNU/BSD make entry points and
builds only the Windows XP-compatible daemon executable with MSVC. It uses the
static C runtime (`/MT`) and links as an x86 console program with a Windows XP
minimum subsystem version.

BSD make has a separate POSIX-only entry point. It intentionally does not expose
the Windows XP cross-build targets:

- `bmake`
- `bmake all-posix`
- `bmake check`
- `bmake check-posix`

Invoke BSD make from this directory, or use `bmake -C
crates/remote-exec-daemon-cpp ...` from the repository root, so the relative
source paths and `build/` output tree resolve correctly. BSD make selects the
top-level `Makefile` automatically.

Focused host-native tests:

Use the same target names with `bmake ...` for the BSD make path.

- `make test-host-patch`
- `make test-host-transfer`
- `make test-host-config`
- `make test-host-http-request`
- `make test-host-session-store`
- `make test-host-server-routes`
- `make test-host-server-streaming`
- `make test-host-sandbox`

## Run

POSIX:

```sh
build/remote-exec-daemon-cpp config/daemon-cpp.example.ini
```

Windows XP-compatible:

```bat
build\remote-exec-daemon-cpp-xp.exe config\daemon-cpp.example.ini
```

Windows XP-compatible MSVC/NMAKE:

```bat
build\remote-exec-daemon-cpp-xp-msvc.exe config\daemon-cpp.example.ini
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

The C++ daemon implements the same daemon-side HTTP/1.1 Upgrade tunnel used by
broker `forward_ports`: TCP listeners/connectors, full-duplex UDP datagram
sockets, non-loopback listen binds, and the same bare-port normalization where
`8080` means `127.0.0.1:8080`. The older lease-renewed port-forward routes are
not exposed.

The v4 frame numbers for `ForwardRecovering` and `ForwardRecovered` are
reserved for compatibility with the Rust protocol table. Current C++ and Rust
implementations report recovery through broker-owned `forward_ports list` state
instead of daemon-emitted recovery frames.

Live forwarded sockets are reconnect-aware in-memory daemon state. When only the
broker-daemon transport drops and the daemon stays alive, the broker may
recover from transport loss or missed heartbeat acknowledgements on either
forwarding side while the daemon retains the forward itself plus future TCP
accepts or future UDP datagrams on the listen side. Active TCP streams and UDP
per-peer connector state are not
preserved across reconnect. C++ forwarding worker threads are capped by
`port_forward_max_worker_threads`; each active forwarded TCP stream uses
separate read and write workers so slow peer writes cannot block the tunnel
control reader. v4 `TunnelReady.limits` truthfully report the configured active
TCP stream, UDP bind, and tunnel queued-byte limits.
Retained sessions, retained listeners, UDP binds, active TCP streams, and
outbound queued tunnel bytes are enforced daemon-wide. Upgraded tunnel socket
reads and writes are bounded by `port_forward_tunnel_io_timeout_ms`, so a peer
that sends an incomplete v4 frame cannot occupy a daemon worker indefinitely.
Outbound TCP connect attempts are bounded by `port_forward_connect_timeout_ms`,
and active TCP stream limits count established streams rather than pending
connect attempts. Recoverable daemon-local pressure drops, such as rejected TCP
accepts or UDP datagrams, are reported back to the broker for public drop
counters. Per-stream TCP connect failures close only that accepted TCP stream
and leave the parent forward open. A broker restart still drops the broker-owned
`forward_id` mapping, and a daemon restart still destroys the forward. If the
broker disappears without reconnecting, the daemon reclaims detached listeners
and UDP sockets after the reconnect grace window expires.
Daemon shutdown closes live forwarded sockets promptly.
Recoverable peer abort/reset errors during forwarding are reported as normal
request failures and do not terminate the daemon process.

`view_image` supports passthrough reads for PNG, JPEG, and WebP only. The
daemon does not resize or re-encode images, so omitted `detail` defaults to
`original`.

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
# C++ forwarding uses detached worker threads for retained listener, UDP,
# reconnect-expiry, and TCP stream work. Each active TCP stream uses separate
# read and write workers.
# port_forward_max_worker_threads = 256
# port_forward_max_retained_sessions = 64
# port_forward_max_retained_listeners = 64
# port_forward_max_udp_binds = 64
# port_forward_max_active_tcp_streams = 1024
# port_forward_max_tunnel_queued_bytes = 8388608
# port_forward_tunnel_io_timeout_ms = 30000
# port_forward_connect_timeout_ms = 10000

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

# Optional static path sandbox. Values are semicolon-separated path lists.
# Missing allow lists, empty allow lists, or omitted sandbox keys mean "allow
# all", then deny lists carve out exclusions.
# sandbox_exec_cwd_allow = /work
# sandbox_exec_cwd_deny = /work/private
# sandbox_read_allow = /work;/assets
# sandbox_read_deny = /work/.git;/assets/secrets
# sandbox_write_allow = /work
# sandbox_write_deny = /work/.git;/work/readonly
```

Sandbox rules mirror the Rust daemon's static allow/deny model:

- `sandbox_exec_cwd_*` applies to the resolved starting `cwd` for `exec_command`.
- `sandbox_read_*` applies to transfer export source paths.
- `sandbox_write_*` applies to transfer import destinations, transfer path-info
  destination probes, and resolved `apply_patch` write targets.
- Empty or omitted `allow` lists allow all paths for that access class; `deny`
  entries override allow membership.
- POSIX roots are canonicalized through existing ancestors, so symlinks in
  configured roots or requested paths cannot bypass boundary checks. Windows
  roots use Windows-style normalization and case-insensitive matching.

## Shell Policy

- POSIX default shell selection follows the Rust daemon's policy: configured
  `default_shell`, then `SHELL`, passwd shell, `bash`, and `/bin/sh`.
- POSIX exec uses `shell -c <cmd>` or `shell -l -c <cmd>` for login shells.
- POSIX child processes currently force `LC_ALL=C.UTF-8` and `LANG=C.UTF-8`.
- Windows XP-compatible exec supports `cmd.exe`; `login=false` adds `/D` before
  `/C`, while `login=true` omits `/D`.

## Limitations

- plain HTTP only, with optional bearer-auth request authentication
- daemon RPC is HTTP/1.1-only; sequential requests may reuse a persistent connection, but HTTP pipelining is not supported
- no TLS support
- PTY support is POSIX-only and depends on host PTY allocation
- no PTY support in Windows XP-compatible builds
- `view_image` supports passthrough PNG, JPEG, and WebP only
- omitted `view_image.detail` defaults to `original` because no resize/re-encode path exists
- broker-owned `forward_id` values do not persist across broker restart
- transient broker-daemon transport drops preserve only the forward itself plus future listen-side TCP accepts or UDP datagrams; active TCP streams and UDP per-peer connector state are lost
- per-stream TCP connect failures close only that accepted TCP stream and leave the parent forward open
- transfer compression is not supported
- `transfer_files` supports regular files, directory trees, and broker-built multi-source bundles
- `transfer_files` accepts an optional export-side `exclude` array. Patterns match paths relative to each source root, use `/` as the logical separator on all platforms, and support `*`, `?`, `**`, `[abc]`, `[a-z]`, `[!abc]`, `[!a-c]`, `[^abc]`, and `[^a-c]`
- excluded matches are silent, excluded directories are pruned recursively, and single-file sources ignore `exclude` in v1
- daemon HTTP transfer imports and exports stream archive bodies instead of staging the full tar payload in memory
- transfer imports support `fail`, `merge`, and `replace` overwrite modes; `merge` overlays compatible existing destinations without deleting unrelated directory entries
- POSIX transfer exports skip unsupported special entries in directory trees and report warnings
- POSIX transfer symlink modes support preserving, following, or skipping symlinks
- Windows XP-compatible transfer builds skip symlink entries inside directory transfers and import archives when preservation is unavailable; follow mode copies regular-file and directory targets when the platform exposes them
- transfer payloads use GNU tar for files and directories
- single-file transfers use the fixed archive entry `.remote-exec-file`
- transfer warnings use the reserved archive summary entry `.remote-exec-transfer-summary.json`, which is consumed during import and is not extracted
- unsupported archive entries remain rejected: hard links, special files unless skipped during export, sparse entries, and malformed paths
- broker targets that point at this daemon must use `http://...` plus `allow_insecure_http = true`
- optional `http_auth_bearer_token` can require `Authorization: Bearer ...` from the broker, but it still does not encrypt plain-HTTP traffic
