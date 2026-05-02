# C++ Exec Concurrency And Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Remove accidental cross-session exec serialization in the C++ daemon and tighten backend stdin handling without changing the public RPC surface.

**Architecture:** `SessionStore` keeps one global mutex only for map ownership and session-limit accounting, while each `LiveSession` gets its own operation mutex for polling, stdin writes, and completion. A lightweight pending-start reservation count keeps `max_open_sessions` honest while `start_command(...)` polls outside the global lock, and backend-specific stdin-close failures are normalized into the existing `stdin_closed` surface.

**Tech Stack:** C++17, POSIX process/PTY APIs, Win32 process/pipe APIs, custom HTTP routing, `make` host tests, XP cross-build checks

---

## File Map

- Modify: `crates/remote-exec-daemon-cpp/include/session_store.h`
  - Add per-session synchronization state and store-level pending-start accounting.
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
  - Narrow the global lock scope, add identity-safe completion/removal, and keep session-limit reservations correct across concurrent starts.
- Modify: `crates/remote-exec-daemon-cpp/include/process_session.h`
  - Add a dedicated backend exception for stdin-closed write failures.
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
  - Preserve the write loop and map `EPIPE` / `EIO` closed-input failures to the dedicated stdin-closed exception.
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
  - Loop `WriteFile(...)` until the full buffer is sent and map broken-pipe-style failures to the dedicated stdin-closed exception.
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
  - Add host-side regressions for cross-session concurrency, late output drain, and newline-preserving token truncation.
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
  - Add a route-level concurrency regression that exercises the same daemon state from two threads.

## Task 1: Refactor `SessionStore` For Real Parallel Session Progress

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/session_store.h`
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `TDD`
Reason: The serialization bug has direct host-side seams in both `SessionStore` and `route_request(...)`, so write the failing regressions first and then refactor the lock model until they pass.

- [ ] **Step 1: Add failing host regressions for unrelated-session concurrency and output behavior**

```cpp
// crates/remote-exec-daemon-cpp/tests/test_session_store.cpp
#include <thread>

#ifndef _WIN32
    const Json late_output = store.start_command(
        "(sleep 0.08; printf 'late tail') &",
        root.string(),
        shell,
        false,
        false,
        true,
        5000UL,
        10UL,
        yield_time,
        64UL
    );
    assert(!late_output.at("running").get<bool>());
    assert(late_output.at("exit_code").get<int>() == 0);
    assert(late_output.at("output").get<std::string>() == "late tail");

    const Json newline_preserved = store.start_command(
        "printf 'one two\\n'",
        root.string(),
        shell,
        false,
        false,
        true,
        5000UL,
        3UL,
        yield_time,
        64UL
    );
    assert(newline_preserved.at("original_token_count").get<unsigned long>() == 2UL);
    assert(newline_preserved.at("output").get<std::string>() == "one two\n");

    if (process_session_supports_pty()) {
        const Json slow_running = store.start_command(
            "printf slow; sleep 30",
            root.string(),
            shell,
            false,
            true,
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        const Json fast_running = store.start_command(
            "IFS= read line; printf '%s' \"$line\"; sleep 30",
            root.string(),
            shell,
            false,
            true,
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );

        Json slow_poll;
        std::thread slow_thread([&]() {
            slow_poll = store.write_stdin(
                slow_running.at("daemon_session_id").get<std::string>(),
                "",
                true,
                5000UL,
                DEFAULT_MAX_OUTPUT_TOKENS,
                yield_time
            );
        });

        platform::sleep_ms(200);
        const std::uint64_t fast_started_at = platform::monotonic_ms();
        const Json fast_completed = store.write_stdin(
            fast_running.at("daemon_session_id").get<std::string>(),
            "ping\n",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time
        );
        const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
        assert(fast_elapsed_ms < 2000UL);
        assert(fast_completed.at("output").get<std::string>().find("ping") != std::string::npos);
        slow_thread.join();
    }
#endif

// crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
#include <thread>

#ifndef _WIN32
    if (process_session_supports_pty()) {
        const Json slow_started = Json::parse(route_request(
            state,
            json_request(
                "/v1/exec/start",
                Json{
                    {"cmd", "printf slow; sleep 30"},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        ).body);
        const Json fast_started = Json::parse(route_request(
            state,
            json_request(
                "/v1/exec/start",
                Json{
                    {"cmd", "IFS= read line; printf '%s' \"$line\"; sleep 30"},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        ).body);

        HttpResponse slow_poll_response;
        std::thread slow_thread([&]() {
            slow_poll_response = route_request(
                state,
                json_request(
                    "/v1/exec/write",
                    Json{
                        {"daemon_session_id", slow_started.at("daemon_session_id").get<std::string>()},
                        {"chars", ""},
                        {"yield_time_ms", 5000},
                    }
                )
            );
        });

        platform::sleep_ms(200);
        const std::uint64_t fast_started_at = platform::monotonic_ms();
        const HttpResponse fast_write_response = route_request(
            state,
            json_request(
                "/v1/exec/write",
                Json{
                    {"daemon_session_id", fast_started.at("daemon_session_id").get<std::string>()},
                    {"chars", "ping\n"},
                    {"yield_time_ms", 250},
                }
            )
        );
        const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
        assert(fast_write_response.status == 200);
        assert(fast_elapsed_ms < 2000UL);
        assert(Json::parse(fast_write_response.body).at("output").get<std::string>().find("ping") != std::string::npos);
        slow_thread.join();
        assert(slow_poll_response.status == 200);
    }
#endif
```

- [ ] **Step 2: Run the focused verification for this step**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-routes
```

Expected:

- `test-host-session-store` FAILS because one session poll still blocks the unrelated session.
- `test-host-server-routes` FAILS for the same reason at the routed `/v1/exec/write` layer.

- [ ] **Step 3: Implement per-session locking, pending-start reservation, and identity-safe removal**

```cpp
// crates/remote-exec-daemon-cpp/include/session_store.h
struct LiveSession {
    LiveSession();
    ~LiveSession();

    BasicMutex mutex_;
    std::string id;
    std::unique_ptr<ProcessSession> process;
    std::uint64_t started_at_ms;
    std::string output_carry;
    bool stdin_open;
    bool retired;
};

class SessionStore {
public:
    SessionStore();
    ~SessionStore();
    Json start_command(
        const std::string& command,
        const std::string& workdir,
        const std::string& shell,
        bool login,
        bool tty,
        bool has_yield_time_ms,
        unsigned long yield_time_ms,
        unsigned long max_output_tokens,
        const YieldTimeConfig& yield_time,
        unsigned long max_open_sessions
    );
    Json write_stdin(
        const std::string& daemon_session_id,
        const std::string& chars,
        bool has_yield_time_ms,
        unsigned long yield_time_ms,
        unsigned long max_output_tokens,
        const YieldTimeConfig& yield_time
    );

private:
    BasicMutex mutex_;
    std::map<std::string, std::shared_ptr<LiveSession> > sessions_;
    unsigned long pending_starts_;
};

// crates/remote-exec-daemon-cpp/src/session_store.cpp
#include <vector>

LiveSession::LiveSession() : started_at_ms(0), stdin_open(false), retired(false) {}

SessionStore::SessionStore() : pending_starts_(0UL) {}

SessionStore::~SessionStore() {
    std::vector<std::shared_ptr<LiveSession> > sessions;
    {
        BasicLockGuard lock(mutex_);
        for (std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.begin();
             it != sessions_.end();
             ++it) {
            sessions.push_back(it->second);
        }
        sessions_.clear();
        pending_starts_ = 0UL;
    }

    for (std::size_t i = 0; i < sessions.size(); ++i) {
        BasicLockGuard session_lock(sessions[i]->mutex_);
        sessions[i]->retired = true;
        if (sessions[i]->process.get() != NULL) {
            sessions[i]->process->terminate();
        }
    }
}

static void erase_session_if_current(
    BasicMutex& store_mutex,
    std::map<std::string, std::shared_ptr<LiveSession> >& sessions,
    const std::string& daemon_session_id,
    const std::shared_ptr<LiveSession>& session
) {
    BasicLockGuard lock(store_mutex);
    std::map<std::string, std::shared_ptr<LiveSession> >::iterator it =
        sessions.find(daemon_session_id);
    if (it != sessions.end() && it->second == session) {
        sessions.erase(it);
    }
}

Json SessionStore::start_command(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login,
    bool tty,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time,
    unsigned long max_open_sessions
) {
    {
        BasicLockGuard lock(mutex_);
        if (sessions_.size() + pending_starts_ >= max_open_sessions) {
            throw SessionLimitError("too many open exec sessions");
        }
        ++pending_starts_;
    }

    std::shared_ptr<LiveSession> session;
    try {
        session = launch_live_session(command, workdir, shell, login, tty);
        const unsigned long timeout_ms = resolve_yield_time_ms(
            yield_time.exec_command,
            has_yield_time_ms,
            yield_time_ms
        );

        PollResult poll_result;
        {
            BasicLockGuard session_lock(session->mutex_);
            poll_result = poll_session(session, timeout_ms);
            if (poll_result.completed) {
                session->retired = true;
            }
        }

        {
            BasicLockGuard lock(mutex_);
            --pending_starts_;
            if (!poll_result.completed) {
                sessions_[session->id] = session;
            }
        }

        if (poll_result.completed) {
            return build_response(
                NULL,
                false,
                session->started_at_ms,
                true,
                poll_result.exit_code,
                poll_result.output,
                max_output_tokens
            );
        }

        return build_response(
            session->id.c_str(),
            true,
            session->started_at_ms,
            false,
            0,
            poll_result.output,
            max_output_tokens
        );
    } catch (...) {
        BasicLockGuard lock(mutex_);
        if (pending_starts_ > 0UL) {
            --pending_starts_;
        }
        throw;
    }
}

Json SessionStore::write_stdin(
    const std::string& daemon_session_id,
    const std::string& chars,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time
) {
    std::shared_ptr<LiveSession> session;
    {
        BasicLockGuard lock(mutex_);
        std::map<std::string, std::shared_ptr<LiveSession> >::iterator it =
            sessions_.find(daemon_session_id);
        if (it == sessions_.end()) {
            throw UnknownSessionError("Unknown daemon session");
        }
        session = it->second;
    }

    PollResult poll_result;
    {
        BasicLockGuard session_lock(session->mutex_);
        if (session->retired) {
            throw UnknownSessionError("Unknown daemon session");
        }
        if (!chars.empty()) {
            if (!session->stdin_open) {
                throw StdinClosedError(
                    "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
                );
            }
            session->process->write_stdin(chars);
        }

        const YieldTimeOperationConfig& operation_config =
            chars.empty() ? yield_time.write_stdin_poll : yield_time.write_stdin_input;
        const unsigned long timeout_ms = resolve_yield_time_ms(
            operation_config,
            has_yield_time_ms,
            yield_time_ms
        );
        poll_result = poll_session(session, timeout_ms);
        if (poll_result.completed) {
            session->retired = true;
        }
    }

    if (!poll_result.completed) {
        return build_response(
            session->id.c_str(),
            true,
            session->started_at_ms,
            false,
            0,
            poll_result.output,
            max_output_tokens
        );
    }

    erase_session_if_current(mutex_, sessions_, daemon_session_id, session);
    return build_response(
        NULL,
        false,
        session->started_at_ms,
        true,
        poll_result.exit_code,
        poll_result.output,
        max_output_tokens
    );
}
```

- [ ] **Step 4: Run the post-change verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-routes
```

Expected:

- both commands PASS
- the new concurrency assertions complete under the 2-second threshold
- the late-output and newline-preservation regressions stay green

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/session_store.h \
        crates/remote-exec-daemon-cpp/src/session_store.cpp \
        crates/remote-exec-daemon-cpp/tests/test_session_store.cpp \
        crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
git commit -m "fix: allow concurrent C++ exec sessions"
```

## Task 2: Tighten Backend Stdin Robustness Without Changing The RPC Surface

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/process_session.h`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** `existing tests + targeted verification`
Reason: The user-visible surface already exists (`stdin_closed`, XP stdin round-trips, PTY input on POSIX). Tightening the backend error mapping and Win32 write loop is best verified through the existing host and XP integration checks rather than by building new fake-process scaffolding.

- [ ] **Step 1: Add a dedicated backend exception for stdin-closed write failures and translate it in the store**

```cpp
// crates/remote-exec-daemon-cpp/include/process_session.h
class ProcessStdinClosedError : public std::runtime_error {
public:
    explicit ProcessStdinClosedError(const std::string& message)
        : std::runtime_error(message) {}
};

// crates/remote-exec-daemon-cpp/src/session_store.cpp
        if (!chars.empty()) {
            if (!session->stdin_open) {
                throw StdinClosedError(
                    "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
                );
            }
            try {
                session->process->write_stdin(chars);
            } catch (const ProcessStdinClosedError& ex) {
                session->stdin_open = false;
                throw StdinClosedError(ex.what());
            }
        }
```

- [ ] **Step 2: Run the focused verification before backend-specific changes**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
```

Expected:

- PASS
- no public error-string changes beyond the existing `stdin is closed for this session; rerun exec_command with tty=true to keep stdin open`

- [ ] **Step 3: Implement POSIX `EPIPE` / `EIO` mapping and Win32 full-buffer writes**

```cpp
// crates/remote-exec-daemon-cpp/src/process_session_posix.cpp
            if (written < 0) {
                if (errno == EINTR) {
                    continue;
                }
                if (errno == EPIPE || errno == EIO) {
                    throw ProcessStdinClosedError(
                        "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
                    );
                }
                throw std::runtime_error(
                    std::string("write(stdin) failed: ") + std::strerror(errno)
                );
            }

// crates/remote-exec-daemon-cpp/src/process_session_win32.cpp
static bool is_stdin_closed_error(DWORD error) {
    return error == ERROR_BROKEN_PIPE ||
           error == ERROR_NO_DATA ||
           error == ERROR_PIPE_NOT_CONNECTED;
}

void write_stdin(const std::string& chars) override {
    const char* data = chars.data();
    std::size_t remaining = chars.size();
    while (remaining > 0U) {
        DWORD written = 0;
        if (WriteFile(
                stdin_write_.get(),
                data,
                static_cast<DWORD>(remaining),
                &written,
                NULL
            ) == 0) {
            const DWORD error = GetLastError();
            if (is_stdin_closed_error(error)) {
                throw ProcessStdinClosedError(
                    "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
                );
            }
            throw std::runtime_error(last_error_message("WriteFile"));
        }
        if (written == 0U) {
            throw std::runtime_error("WriteFile wrote zero bytes");
        }
        data += written;
        remaining -= static_cast<std::size_t>(written);
    }
}
```

- [ ] **Step 4: Run the post-change verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected:

- `test-host-session-store` PASS
- `check-posix` PASS
- `check-windows-xp` PASS
- existing stdin-closed behavior remains stable while the Win32 backend now handles multi-write input more defensively

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/process_session.h \
        crates/remote-exec-daemon-cpp/src/process_session_posix.cpp \
        crates/remote-exec-daemon-cpp/src/process_session_win32.cpp \
        crates/remote-exec-daemon-cpp/src/session_store.cpp
git commit -m "fix: harden C++ exec stdin handling"
```
