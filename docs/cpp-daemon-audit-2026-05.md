# C++ Daemon Audit — May 2026

Audit of `crates/remote-exec-daemon-cpp/` for lifetime, lifecycle, and
BSD/POSIX compatibility issues. Context: two port-forwarding bugs and one
BSD-specific bug were found and fixed in recent commits (`cea01ee`, `ba0ef27`,
`e3551dd`). This audit covers the rest of the daemon.

## Fixed Issues

### HIGH — EINTR/ECONNABORTED kills tcp_accept_loop permanently

`src/port_tunnel_tcp.cpp:65-78`

When `accept()` fails, only EAGAIN/EWOULDBLOCK triggers a retry. On BSD,
ECONNABORTED occurs when a client RSTs before accept completes. On all POSIX,
EINTR occurs on signal delivery (e.g. SIGCHLD). Both permanently terminate the
listener thread, killing all future accepts on that port forward.

Same EINTR gap in `udp_read_loop` and `udp_read_loop_connection_local`
(`port_tunnel_udp.cpp`).

### HIGH — Data race on reaped_/exit_code_

`src/process_session_posix.cpp:540-596`

`PosixProcessSession::reaped_` and `exit_code_` are plain `bool`/`int` accessed
concurrently by the pump thread (`output_may_resume`) and request handler
threads (`has_exited`, `terminate`) without synchronization. Undefined behavior
under C++11. Benign on x86 but incorrect on weaker memory architectures.

### MEDIUM — PTY master fd missing O_CLOEXEC

`src/process_session_posix.cpp:169`

`posix_openpt(O_RDWR | O_NOCTTY)` doesn't set CLOEXEC. Concurrent
`launch(tty=true)` calls can leak the master fd into each other's children,
preventing EOF detection on the pty.

### MEDIUM — ptsname thread safety on BSD

`src/process_session_posix.cpp:186-192`

The non-glibc branch calls `ptsname()` which returns a static buffer.
Concurrent calls can corrupt the result. FreeBSD 12+ has `ptsname_r` but the
guard only checks `__GLIBC__`.

### MEDIUM — Missing FD_CLOEXEC on accepted port-forward sockets

`src/port_tunnel_tcp.cpp:63`

`accept()` in `tcp_accept_loop` doesn't set CLOEXEC. Accepted sockets leak
into forked child processes.

### LOW — POLLHUP not checked in wait_for_connect

`src/port_forward_socket_ops.cpp:146`

On some BSDs, a refused non-blocking connect reports POLLHUP without POLLOUT.
The function returns false, producing a misleading "timed out" error instead of
connection-refused.

### LOW — readdir error not distinguished from end-of-directory

`src/transfer_ops_fs.cpp:252-264`

POSIX `readdir` returns NULL for both end-of-directory and error. Without
setting errno=0 before and checking after, a mid-traversal error silently
produces a partial listing.

## Documented But Not Fixed

These require larger architectural changes:

### MEDIUM — Stale fd TOCTOU race (5 sites)

`port_tunnel_tcp.cpp:28-36,182-183,230` and `port_tunnel_udp.cpp:34-41,191-199`

Raw fds are extracted under a lock then used for I/O after the lock is released.
Between unlock and syscall, another thread can close the socket and the OS can
recycle the fd number. The shutdown-before-close pattern + POLLNVAL check + 100ms
poll timeout mitigates but does not eliminate the race.

Fix requires a wakeup pipe/eventfd architecture to avoid polling the socket fd
directly.

### MEDIUM — BSD shutdown() on listener/UDP sockets is a no-op

`port_tunnel.cpp` (RetainedTcpListener::close, TunnelUdpSocket::close)

`shutdown(SHUT_RDWR)` on non-connected sockets returns ENOTCONN on BSD with no
effect. The blocking thread is only woken when the subsequent `close()` fires.
Creates up to 100ms extra shutdown latency and a race window for orphaned
connections.

Same root cause as the TOCTOU issue — needs wakeup mechanism.

### LOW — CLOCK_REALTIME in condvar timed waits

`src/basic_mutex_posix.cpp:33-43`

`timed_wait_ms` uses CLOCK_REALTIME. NTP clock steps can extend or shorten
waits. Fix requires `pthread_condattr_setclock(CLOCK_MONOTONIC)` which has
portability concerns on older OpenBSD.

### LOW — SO_REUSEADDR BSD semantics

`src/port_forward_socket_ops.cpp:224`

On BSD, SO_REUSEADDR allows two sockets to bind the same address:port
simultaneously (port hijacking). On Linux it only allows binding TIME_WAIT
addresses. Informational — no code change planned.

### LOW — Glob exponential backtracking

`src/transfer_glob.cpp:77-146`

Naive recursive backtracking. A crafted exclude pattern can cause CPU
exhaustion. Needs algorithm rewrite (NFA or memoization).

### LOW — No clean shutdown mechanism

`src/server.cpp:29-53`

No SIGTERM/SIGINT handler calls `request_shutdown()`. The ServerRuntime
destructor cleanup is effectively dead code. Daemon relies on OS cleanup on
signal death.

### LOW — Detached reaper thread

`src/posix_child_reaper.cpp:161`

The reaper thread is detached with no exit condition. If static destructors
ever run, it accesses destroyed globals. Coupled to the shutdown mechanism.

### LOW — Transfer import chmod TOCTOU

`src/transfer_ops_import.cpp:276-282`

After closing a written file, `stat_path` (follows symlinks) + `chmod` is
called on the same path. A symlink placed between close and chmod causes
execute bits to be added to an arbitrary file. Needs restructuring to use
`fchmod` before close.

### LOW — Socket fd leak on std::bad_alloc (4 sites)

`port_tunnel_tcp.cpp:111-112,158-159` and `port_tunnel_udp.cpp:124-125`

`UniqueSocket::release()` is called in a `new` expression. If allocation
throws, the fd is leaked. Rare in practice (bad_alloc is uncommon) but
technically a resource leak.

### LOW — wait_posix_child_exit blocks under global mutex

`src/posix_child_reaper.cpp:211-217`

`waitpid(pid, status, 0)` is called under `g_mutex`, blocking all concurrent
child management for the duration. Can cause latency spikes during session
pruning.

## Round 2 — Fixed Issues

### MEDIUM-HIGH — ensure_raw_line unbounded buffer growth (DoS)

`src/server_transport.cpp:241-257`

When reading chunked bodies, `ensure_raw_line()` loops calling `recv` and
appending to `raw_` until it finds `\r\n`. A malicious client can send a
continuous stream of non-CRLF bytes, growing `raw_` without bound and
exhausting memory. The `max_body_bytes_` check only fires after the chunk size
is parsed, not during the line read itself. Same issue in
`consume_chunk_trailers()`.

### MEDIUM — No file size limit in image_read

`src/server_route_image.cpp:40-65`

`read_binary_file_bytes` reads an entire file into memory with no size cap. A
request pointing to a large file causes unbounded allocation. The response then
base64-encodes it, amplifying memory usage by ~33%.

### LOW — Negative st_size wraps to huge uint64 in tar export

`src/transfer_ops_tar.cpp:223`

`static_cast<std::uint64_t>(st.st_size)` wraps negative values to near-UINT64_MAX,
causing the export to emit a bogus tar header and attempt an enormous read.

### LOW — Missing chunked terminator on export error after headers sent

`src/http_connection.cpp:220-259`

When `export_path_to_sink_as` throws after chunked response headers are sent,
the chunked terminator (`0\r\n\r\n`) is never written. The HTTP response stream
is left in an invalid state.

### LOW — Integer overflow in output_renderer on 32-bit

`src/output_renderer.cpp:81`

`max_output_tokens * BYTES_PER_TOKEN` can overflow `size_t` on 32-bit platforms
when `max_output_tokens` is large.

## Round 2 — Documented But Not Fixed

### MEDIUM — Symlink TOCTOU races in tar import

`src/transfer_ops_import.cpp:127-139,448` and `src/transfer_ops_fs.cpp:96-124,195-213`

Multiple TOCTOU races between symlink checks and file writes. Between
validation and use, a local attacker could swap a directory for a symlink,
causing writes to land outside the destination. Fixing requires `openat()`-style
directory-fd-relative operations.

### MEDIUM — Patch engine does not verify symlink-free path

`src/patch_engine.cpp:262-290`

`write_text_atomic` creates parent directories and writes via temp+rename
without checking whether path components are symlinks. The
`PatchPathAuthorizer` validates the logical path but not the physical state.

### LOW-MEDIUM — Quadratic header search in try_read_http_request_head

`src/server_transport.cpp:90-132`

Each recv appends to a buffer then searches from the beginning for `\r\n\r\n`.
With max_header_bytes=64KB and 1-byte-at-a-time delivery, this is O(n^2) CPU.
Bounded by max_header_bytes and per-connection threading.

### LOW-MEDIUM — Zero body-read timeout enables slowloris on body

`src/http_connection.cpp:318`

After reading headers, socket timeout is set to 0 (infinite). A client sending
headers with large Content-Length then stopping holds the connection thread
indefinitely. Combined with no per-IP limit, 64 such connections block the
daemon.

### LOW — Potential std::terminate if exception after start_session_pump

`src/session_store.cpp:420+`

If `wait_for_session_activity` throws after `start_session_pump`, no
`join_session_pump` is called. The pump thread's last `shared_ptr` drop
destroys a joinable `std::thread`, calling `std::terminate()`.

