# C++ Daemon Architecture Audit - 2026-05-19

Status: follow-up queued

Scope: `crates/remote-exec-daemon-cpp/`

This records the architecture and compatibility review performed after BSD
testing exposed two C++ daemon port-forwarding bugs and recent commits fixed
another BSD-specific issue.

Constraints for follow-up work:

- Preserve the thread-based concurrency model.
- Preserve the no-`openat` design.
- Prioritize BSD and other POSIX compatibility fixes first.
- Ignore the `LC_ALL=C.UTF-8` / `LANG=C.UTF-8` portability concern for now.
- Defer the IPv4-only daemon listener limitation for now.
- Commit after each completed task.

## Findings

### High - Recursive Transfer Sandbox Checks Are Root-Only

Transfer export authorizes only the requested root path, then recursively walks
children without checking each materialized child path against read sandbox
rules. With `sandbox_read.allow=/work` and
`sandbox_read.deny=/work/secret`, exporting `/work` can include
`/work/secret`. With symlink mode `follow`, a symlink inside an allowed tree can
also pull data from outside the allowed tree.

Transfer import has the symmetric write-side issue. The destination root is
authorized once, but regular files and directories inside a directory or
multiple-source archive are materialized without per-entry write authorization.
With `sandbox_write.allow=/work` and `sandbox_write.deny=/work/secret`,
importing an archive containing `secret/file.txt` to `/work` can write under the
denied subtree. `overwrite=replace` can also remove denied children when
deleting the authorized destination root.

Relevant C++ locations:

- `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
- `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`
- `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`

This is compatible with the no-`openat` constraint if the existing path
authorizer is threaded through export recursion, import materialization, and
recursive deletion. The remaining TOCTOU risk from path-based checks is an
accepted consequence of the current design unless the security model changes.

### High - Streaming Export Can Report Success For Partial Archives

The streaming export route sends HTTP 200 and transfer headers before walking
the source tree. If export fails after headers have been sent, the handler
terminates the chunked response and still reports success. The import side
currently accepts EOF at a tar block boundary without requiring an explicit tar
terminator, so a source-side error after several complete entries can look like
a valid shorter archive.

Relevant C++ locations:

- `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`

Potential follow-up options are export preflight before headers, stricter tar
terminator validation on import, or a transfer-level streamed error signal if
the broker-daemon contract supports one.

### Medium - Interrupted Streaming Import Leaves Partial Files

Transfer import writes directly to the final destination path while reading the
archive stream. If the request body is truncated or the connection is
interrupted mid-file, the partial destination file can remain in place.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`

A temp-file-then-rename path with cleanup on failure would preserve the current
threaded and no-`openat` design.

### Medium - TCP Port-Forward Reads Do Not Retry EINTR

UDP read loops and HTTP/tunnel frame reads handle `EINTR`, but
`tcp_read_loop()` treats any negative `recv()` as a hard port read failure.
Signal delivery on BSD or other POSIX systems can therefore close otherwise
healthy TCP streams.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`

### Medium/Low - Control Frames Have No Independent Queue Cap

Port tunnel control, error, heartbeat, and drop frames are intentionally
uncharged so they can pass data-frame backpressure. The queue has no separate
frame-count or byte cap for uncharged frames, so a stalled peer plus repeated
control/error generation can grow memory outside `max_tunnel_queued_bytes`.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/port_tunnel_sender.cpp`

### Low - POSIX Timeout Loops Restart Full Timeout After EINTR

Several `poll()` loops retry after `EINTR` with the original timeout instead of
using a monotonic deadline. Under repeated signal delivery, configured
timeouts can stretch indefinitely.

Relevant C++ locations:

- `crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp`
- `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`

### Low - Nonblocking UDP sendto Treats EAGAIN As Fatal

Retained UDP sockets are nonblocking. Broker-originated datagrams use one
`sendto()` call and treat any failure, including transient `EAGAIN`, as a port
write failure. BSD socket buffer behavior can make this easier to hit under
pressure.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`

## BSD / POSIX Compatibility Watchlist

### pthread_condattr_setclock Availability

`BasicCondVar` assumes `pthread_condattr_setclock()` is available on every
non-Apple POSIX target. This should be feature-checked rather than selected by
OS name. Some BSD or older POSIX libc combinations may expose
`CLOCK_MONOTONIC` without this pthread API.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/basic_mutex_posix.cpp`

### O_CLOEXEC Fallbacks

`open_dev_null_read()` uses `O_CLOEXEC` directly. Modern BSDs usually provide
it, but the daemon already uses `open()` plus `fcntl(FD_CLOEXEC)` fallbacks in
other non-Linux POSIX paths. `/dev/null` should use the same pattern.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`

### PTY API Availability

PTY support assumes UNIX98 APIs: `posix_openpt()`, `grantpt()`, `unlockpt()`,
and `ptsname()` / `ptsname_r()`. Broader POSIX/BSD support should either
feature-gate PTY support to report `supports_pty=false` when unavailable or use
an `openpty()` fallback where available.

Relevant C++ location:

- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`

### Deferred Items

The locale environment (`LC_ALL` / `LANG`) and IPv4-only daemon listener notes
are intentionally deferred by direction and are not part of the immediate
compatibility queue.
