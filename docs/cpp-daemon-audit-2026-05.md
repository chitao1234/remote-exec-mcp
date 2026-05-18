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
