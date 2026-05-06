# C++ Hybrid Supervised Blocking Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Rework the C++ daemon internals into a supervised blocking concurrency model that preserves XP and older POSIX compatibility while removing detached connection threads and sleep-driven session coordination.

**Architecture:** Add one cross-platform wait primitive, convert exec sessions to output-pump plus wakeup-driven coordination, introduce an explicit connection manager for HTTP worker ownership, wrap server startup and shutdown in a runtime owner, and unify port-forward close paths so blocked operations exit through managed shutdown instead of ad hoc polling. Keep the public HTTP/RPC behavior unchanged and verify it through focused host tests plus the existing XP-compatible build path.

**Tech Stack:** C++11 production target, C++17 host tests, POSIX `pthread` / poll / socketpair test seams, Windows XP-compatible Win32 synchronization and thread APIs, existing `Makefile`-driven host and XP checks under `crates/remote-exec-daemon-cpp`

---

## File Structure

- `crates/remote-exec-daemon-cpp/include/basic_mutex.h`
  - Extend the low-level synchronization layer with `BasicCondVar` while keeping `BasicMutex` as the common lock primitive.

- `crates/remote-exec-daemon-cpp/src/basic_mutex.cpp`
  - Implement POSIX `pthread_cond_t` waits and XP-compatible event-backed waits behind the same API.

- `crates/remote-exec-daemon-cpp/tests/test_basic_mutex.cpp`
  - New direct unit coverage for timeout, signal, and broadcast semantics of the new wait primitive.

- `crates/remote-exec-daemon-cpp/include/process_session.h`
  - Add the blocking output-read seam needed by session output pumps without changing the public session store RPC surface.

- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
  - Implement blocking output reads and EOF signaling for the POSIX backend while preserving current stdin and locale behavior.

- `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
  - Implement the same output-pump seam for the Win32 backend while preserving XP-compatible stdin semantics.

- `crates/remote-exec-daemon-cpp/include/session_store.h`
  - Extend `LiveSession` with buffered output state, wakeup state, output-pump ownership, and closing flags.

- `crates/remote-exec-daemon-cpp/src/session_store.cpp`
  - Replace active polling with wakeup-driven session waiting, manage output pump threads, and keep response shaping unchanged.

- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
  - Preserve public exec behavior, same-session serialization, and concurrent-session behavior while proving the refactor did not regress output capture.

- `crates/remote-exec-daemon-cpp/include/connection_manager.h`
  - New internal ownership layer for active HTTP worker threads and client sockets.

- `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
  - Implement admission control, worker tracking, shutdown close paths, and deterministic reaping for client workers.

- `crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp`
  - New direct coverage for connection-cap enforcement, worker reaping, and manager-driven shutdown.

- `crates/remote-exec-daemon-cpp/include/server_runtime.h`
  - New top-level runtime owner for listener lifecycle, shutdown ordering, and background maintenance.

- `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
  - Construct `AppState`, own listener and maintenance threads, and drive shutdown plus periodic sweeps.

- `crates/remote-exec-daemon-cpp/include/server.h`
  - Keep `handle_client(...)` and `run_server(...)` stable while making room for runtime-owned startup helpers.

- `crates/remote-exec-daemon-cpp/src/server.cpp`
  - Shrink to daemon bootstrapping and accept-loop delegation through `ServerRuntime` and `ConnectionManager`.

- `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
  - Preserve blocking per-connection routing while adding explicit idle-timeout handling and manager-driven exit behavior.

- `crates/remote-exec-daemon-cpp/include/server_transport.h`
  - Add any socket-timeout helpers needed by the connection worker without pushing policy into unrelated route code.

- `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
  - Implement the timeout helper used by connection workers.

- `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
  - New integration coverage for runtime shutdown, active-connection cleanup, and maintenance-driven reaping.

- `crates/remote-exec-daemon-cpp/include/port_forward.h`
  - Make lifecycle state explicit on listeners and TCP connections.

- `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
  - Unify close paths, make lease expiry and explicit close idempotent, and ensure blocked operations unwind as normal close events.

- `crates/remote-exec-daemon-cpp/tests/test_port_forward.cpp`
  - New direct coverage for close-during-accept, close-during-read, and lease-expiry cleanup.

- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
  - Keep persistent-request and streaming behavior covered while the worker supervision model changes underneath it.

- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
  - Preserve route-level behavior while the HTTP server and port-forward internals are restructured.

- `crates/remote-exec-daemon-cpp/Makefile`
  - Add new source files and host-test targets, then fold those targets into the existing POSIX quality gate.

### Task 1: Add The Cross-Platform Wait Primitive

**Files:**
- Create: `crates/remote-exec-daemon-cpp/tests/test_basic_mutex.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/basic_mutex.h`
- Modify: `crates/remote-exec-daemon-cpp/src/basic_mutex.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-basic-mutex`

**Testing approach:** `TDD`
Reason: the wait primitive is a small, direct seam with deterministic timeout and wakeup behavior that should be proven independently before session and runtime code depends on it.

- [ ] **Step 1: Add the failing unit test target and test cases for timeout, signal, and broadcast**

```cpp
// tests/test_basic_mutex.cpp
#include <cassert>
#include <thread>
#include <vector>

#include "basic_mutex.h"
#include "platform.h"

int main() {
    BasicMutex mutex;
    BasicCondVar cond;
    bool ready = false;

    std::thread waiter([&]() {
        BasicLockGuard lock(mutex);
        while (!ready) {
            const bool woke = cond.timed_wait_ms(mutex, 500UL);
            assert(woke);
        }
    });

    platform::sleep_ms(50);
    {
        BasicLockGuard lock(mutex);
        ready = true;
        cond.signal();
    }
    waiter.join();

    {
        BasicLockGuard lock(mutex);
        const std::uint64_t start = platform::monotonic_ms();
        const bool woke = cond.timed_wait_ms(mutex, 75UL);
        const std::uint64_t elapsed = platform::monotonic_ms() - start;
        assert(!woke);
        assert(elapsed >= 50UL);
    }

    int released = 0;
    ready = false;
    std::vector<std::thread> waiters;
    for (int i = 0; i < 2; ++i) {
        waiters.push_back(std::thread([&]() {
            BasicLockGuard lock(mutex);
            while (!ready) {
                const bool woke = cond.timed_wait_ms(mutex, 500UL);
                assert(woke);
            }
            ++released;
        }));
    }
    platform::sleep_ms(50);
    {
        BasicLockGuard lock(mutex);
        ready = true;
        cond.broadcast();
    }
    for (std::size_t i = 0; i < waiters.size(); ++i) {
        waiters[i].join();
    }
    assert(released == 2);
}
```

```make
# Makefile additions
HOST_BASIC_MUTEX := $(BUILD_DIR)/test_basic_mutex
HOST_BASIC_MUTEX_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_basic_mutex.cpp src/basic_mutex.cpp src/platform.cpp)
HOST_BASIC_MUTEX_OBJS := $(sort $(call host_test_objs,$(HOST_BASIC_MUTEX_SRCS)))
test-host-basic-mutex: $(HOST_BASIC_MUTEX)
	$(HOST_BASIC_MUTEX)

$(HOST_BASIC_MUTEX): $(HOST_BASIC_MUTEX_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^
```

- [ ] **Step 2: Run the new target before implementing `BasicCondVar`**

Run: `make -C crates/remote-exec-daemon-cpp test-host-basic-mutex`
Expected: FAIL to compile because `BasicCondVar` and `timed_wait_ms(...)` do not exist yet.

- [ ] **Step 3: Implement `BasicCondVar` in the portability layer**

```cpp
// include/basic_mutex.h
class BasicCondVar;

class BasicMutex {
public:
    BasicMutex();
    ~BasicMutex();

    void lock();
    void unlock();

private:
    friend class BasicCondVar;
#ifdef _WIN32
    CRITICAL_SECTION mutex_;
#else
    pthread_mutex_t mutex_;
#endif
};

class BasicCondVar {
public:
    BasicCondVar();
    ~BasicCondVar();

    void wait(BasicMutex& mutex);
    bool timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms);
    void signal();
    void broadcast();

    BasicCondVar(const BasicCondVar&) = delete;
    BasicCondVar& operator=(const BasicCondVar&) = delete;

private:
#ifdef _WIN32
    HANDLE signal_event_;
    HANDLE broadcast_event_;
    long waiters_;
#else
    pthread_cond_t cond_;
#endif
};
```

```cpp
// src/basic_mutex.cpp
bool BasicCondVar::timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms) {
#ifdef _WIN32
    InterlockedIncrement(&waiters_);
    mutex.unlock();
    const HANDLE handles[2] = {signal_event_, broadcast_event_};
    const DWORD result = WaitForMultipleObjects(2, handles, FALSE, timeout_ms);
    mutex.lock();
    InterlockedDecrement(&waiters_);
    return result == WAIT_OBJECT_0 || result == WAIT_OBJECT_0 + 1;
#else
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_sec += static_cast<time_t>(timeout_ms / 1000UL);
    deadline.tv_nsec += static_cast<long>((timeout_ms % 1000UL) * 1000000UL);
    if (deadline.tv_nsec >= 1000000000L) {
        ++deadline.tv_sec;
        deadline.tv_nsec -= 1000000000L;
    }
    return pthread_cond_timedwait(&cond_, &mutex.mutex_, &deadline) == 0;
#endif
}
```

- [ ] **Step 4: Re-run the focused wait-primitive verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-basic-mutex`
Expected: PASS with signal, broadcast, and timeout behavior covered in one direct unit target.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/tests/test_basic_mutex.cpp \
        crates/remote-exec-daemon-cpp/include/basic_mutex.h \
        crates/remote-exec-daemon-cpp/src/basic_mutex.cpp \
        crates/remote-exec-daemon-cpp/Makefile
git commit -m "refactor: add cpp wait primitive"
```

### Task 2: Replace Session Polling With Output Pumps And Wakeups

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/process_session.h`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/session_store.h`
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-wine-session-store`

**Testing approach:** `characterization/integration test`
Reason: the public exec behavior should stay the same, so the right tests capture and preserve current behavior while the internal coordination model changes underneath it.

- [ ] **Step 1: Extend the session-store characterization tests around preserved output and cross-session behavior**

```cpp
// tests/test_session_store.cpp
const YieldTimeConfig fast_yield = fast_yield_time_config();

const Json delayed = start_test_command(
    store,
    "(sleep 0.05; printf 'delayed') &",
    root.string(),
    shell,
    false,
    5000UL,
    DEFAULT_MAX_OUTPUT_TOKENS,
    fast_yield,
    64UL
);
assert(!delayed.at("running").get<bool>());
assert(delayed.at("output").get<std::string>() == "delayed");

if (process_session_supports_pty()) {
    const Json waiting = start_test_command(
        store,
        "IFS= read line; printf 'echo:%s' \"$line\"; sleep 1",
        root.string(),
        shell,
        true,
        100UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        64UL
    );
    assert(waiting.at("running").get<bool>());
    const std::string id = waiting.at("daemon_session_id").get<std::string>();
    const Json resumed = store.write_stdin(id, "ready\n", true, 1000UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield);
    assert(resumed.at("output").get<std::string>().find("echo:ready") != std::string::npos);
}
```

- [ ] **Step 2: Run the current session-store coverage before changing the implementation**

Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
Expected: PASS, proving the new characterization cases describe preserved current behavior rather than a new public contract.

- [ ] **Step 3: Move `SessionStore` to output-pump driven coordination**

```cpp
// include/process_session.h
virtual std::string read_output(bool block, bool* eof, std::string* carry) = 0;
```

```cpp
// include/session_store.h
struct SessionOutputState {
    std::string buffered_output;
    std::string decode_carry;
    bool eof;
    bool exited;
    int exit_code;
    std::uint64_t generation;
};

struct LiveSession {
    LiveSession();
    ~LiveSession();

    BasicMutex mutex_;
    BasicCondVar cond_;
    std::string id;
    std::unique_ptr<ProcessSession> process;
    std::uint64_t started_at_ms;
    std::atomic<std::uint64_t> last_touched_order;
    SessionOutputState output_;
    bool stdin_open;
    bool retired;
    bool closing;
    bool pump_started;
#ifdef _WIN32
    HANDLE pump_thread;
#else
    std::thread* pump_thread;
#endif
};
```

```cpp
// src/session_store.cpp
static void pump_session_output(const std::shared_ptr<LiveSession>& session) {
    for (;;) {
        bool eof = false;
        std::string chunk;
        {
            BasicLockGuard lock(session->mutex_);
            if (session->closing) {
                break;
            }
        }
        chunk = session->process->read_output(true, &eof, &session->output_.decode_carry);
        {
            BasicLockGuard lock(session->mutex_);
            if (!chunk.empty()) {
                session->output_.buffered_output += chunk;
                ++session->output_.generation;
                session->cond_.broadcast();
            }
            if (eof) {
                session->output_.eof = true;
                int exit_code = 0;
                session->output_.exited = session->process->has_exited(&exit_code);
                session->output_.exit_code = exit_code;
                ++session->output_.generation;
                session->cond_.broadcast();
                break;
            }
        }
    }
}
```

- [ ] **Step 4: Run the preserved-behavior session tests on POSIX and XP-compatible session code**

Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
Expected: PASS with unchanged host session behavior.

Run: `make -C crates/remote-exec-daemon-cpp test-wine-session-store`
Expected: PASS, confirming the new session coordination still works against the XP-compatible backend.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/process_session.h \
        crates/remote-exec-daemon-cpp/src/process_session_posix.cpp \
        crates/remote-exec-daemon-cpp/src/process_session_win32.cpp \
        crates/remote-exec-daemon-cpp/include/session_store.h \
        crates/remote-exec-daemon-cpp/src/session_store.cpp \
        crates/remote-exec-daemon-cpp/tests/test_session_store.cpp
git commit -m "refactor: make cpp exec waiting event-driven"
```

### Task 3: Introduce A Manager For HTTP Worker Ownership

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/connection_manager.h`
- Create: `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
- Create: `crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-connection-manager`

**Testing approach:** `TDD`
Reason: worker admission, deterministic reaping, and shutdown-driven socket closure are direct manager semantics that can be proven before the real server uses them.

- [ ] **Step 1: Add a failing direct test target for connection-cap and shutdown behavior**

```cpp
// tests/test_connection_manager.cpp
#include <cassert>
#include <thread>
#include <unistd.h>

#include <sys/socket.h>

#include "connection_manager.h"
#include "platform.h"

static void hold_worker(SOCKET socket, void* raw_flag) {
    bool* release = static_cast<bool*>(raw_flag);
    while (!*release) {
        platform::sleep_ms(10);
    }
    close(socket);
}

int main() {
    ConnectionManager manager(1UL);
    int pair_one[2];
    int pair_two[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair_one) == 0);
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair_two) == 0);

    bool release_first = false;
    UniqueSocket first(pair_one[0]);
    UniqueSocket second(pair_two[0]);

    assert(manager.try_start(std::move(first), &hold_worker, &release_first));
    assert(manager.active_count() == 1UL);
    assert(!manager.try_start(std::move(second), &hold_worker, &release_first));

    manager.begin_shutdown();
    release_first = true;
    manager.reap_finished();
}
```

```make
# Makefile additions
HOST_CONNECTION_MANAGER := $(BUILD_DIR)/test_connection_manager
HOST_CONNECTION_MANAGER_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_connection_manager.cpp src/connection_manager.cpp src/basic_mutex.cpp src/platform.cpp src/logging.cpp)
HOST_CONNECTION_MANAGER_OBJS := $(sort $(call host_test_objs,$(HOST_CONNECTION_MANAGER_SRCS)))
test-host-connection-manager: $(HOST_CONNECTION_MANAGER)
	$(HOST_CONNECTION_MANAGER)

$(HOST_CONNECTION_MANAGER): $(HOST_CONNECTION_MANAGER_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^
```

- [ ] **Step 2: Run the target before `ConnectionManager` exists**

Run: `make -C crates/remote-exec-daemon-cpp test-host-connection-manager`
Expected: FAIL to compile because `ConnectionManager` and `try_start(...)` have not been introduced yet.

- [ ] **Step 3: Implement the manager-owned worker lifecycle**

```cpp
// include/connection_manager.h
typedef void (*ConnectionWorkerMain)(SOCKET socket, void* context);

class ConnectionManager {
public:
    explicit ConnectionManager(unsigned long max_active_connections);
    ~ConnectionManager();

    bool try_start(UniqueSocket client, ConnectionWorkerMain worker_main, void* context);
    void begin_shutdown();
    void reap_finished();
    unsigned long active_count() const;

private:
    struct WorkerRecord;
    BasicMutex mutex_;
    std::map<unsigned long, std::shared_ptr<WorkerRecord> > workers_;
    unsigned long max_active_connections_;
    bool shutting_down_;
    unsigned long next_worker_id_;
};
```

```cpp
// src/connection_manager.cpp
bool ConnectionManager::try_start(UniqueSocket client, ConnectionWorkerMain worker_main, void* context) {
    BasicLockGuard lock(mutex_);
    if (shutting_down_ || workers_.size() >= max_active_connections_) {
        return false;
    }
    const unsigned long worker_id = next_worker_id_++;
    std::shared_ptr<WorkerRecord> record(new WorkerRecord(worker_id, client.release(), worker_main, context));
    workers_[worker_id] = record;
    record->start();
    return true;
}
```

- [ ] **Step 4: Run the focused manager verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-connection-manager`
Expected: PASS with direct coverage for admission control and orderly worker reaping.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/connection_manager.h \
        crates/remote-exec-daemon-cpp/src/connection_manager.cpp \
        crates/remote-exec-daemon-cpp/tests/test_connection_manager.cpp \
        crates/remote-exec-daemon-cpp/Makefile
git commit -m "refactor: add cpp connection manager"
```

### Task 4: Wrap The Listener In `ServerRuntime` And Supervised HTTP Workers

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/server_runtime.h`
- Create: `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
- Create: `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server.h`
- Modify: `crates/remote-exec-daemon-cpp/include/server_transport.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** `TDD`
Reason: runtime shutdown ordering and worker-owned keepalive sockets are observable through focused runtime tests and existing streaming HTTP integration tests.

- [ ] **Step 1: Add a failing runtime test target for accept-loop shutdown and active-worker cleanup**

```cpp
// tests/test_server_runtime.cpp
#include <cassert>
#include <filesystem>
#include <thread>

#include "server_runtime.h"

int main() {
    const std::filesystem::path root = std::filesystem::temp_directory_path() / "remote-exec-cpp-server-runtime-test";
    std::filesystem::remove_all(root);
    std::filesystem::create_directories(root);

    DaemonConfig config;
    config.target = "cpp-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = root.string();
    config.default_shell.clear();
    config.allow_login_shell = true;
    config.max_request_header_bytes = 65536;
    config.max_request_body_bytes = 65536;
    config.max_open_sessions = 64;
    config.yield_time = default_yield_time_config();

    ServerRuntime runtime(config);
    runtime.start_accept_loop();
    assert(runtime.bound_port() != 0);
    runtime.request_shutdown();
    runtime.join();
}
```

```make
# Makefile additions
HOST_SERVER_RUNTIME := $(BUILD_DIR)/test_server_runtime
HOST_SERVER_RUNTIME_SRCS := $(MAKEFILE_DIR)tests/test_server_runtime.cpp $(addprefix $(MAKEFILE_DIR),src/server_runtime.cpp src/connection_manager.cpp src/server.cpp src/http_connection.cpp src/server_transport.cpp src/http_request.cpp src/http_codec.cpp src/http_helpers.cpp src/session_store.cpp src/process_session_posix.cpp src/platform.cpp src/shell_policy.cpp src/basic_mutex.cpp src/logging.cpp src/config.cpp src/text_utils.cpp src/server_request_utils.cpp) $(ROUTE_SRCS) $(TRANSFER_SRCS) $(POLICY_SRCS) $(RPC_FAILURE_SRCS) $(PORT_FORWARD_SRCS)
HOST_SERVER_RUNTIME_OBJS := $(sort $(call host_test_objs,$(HOST_SERVER_RUNTIME_SRCS)))
test-host-server-runtime: $(HOST_SERVER_RUNTIME)
	$(HOST_SERVER_RUNTIME)

$(HOST_SERVER_RUNTIME): $(HOST_SERVER_RUNTIME_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^
```

- [ ] **Step 2: Run the new runtime target before introducing `ServerRuntime`**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
Expected: FAIL to compile because `ServerRuntime`, `start_accept_loop()`, and `request_shutdown()` do not exist yet.

- [ ] **Step 3: Implement `ServerRuntime`, wire `run_server(...)`, and give workers an explicit idle timeout**

```cpp
// include/server_runtime.h
class ServerRuntime {
public:
    explicit ServerRuntime(const DaemonConfig& config);
    ~ServerRuntime();

    void start_accept_loop();
    void request_shutdown();
    void join();
    unsigned short bound_port() const;
    AppState& state();
    ConnectionManager& connection_manager();
    void maintenance_once();

private:
    void accept_loop();
    void maintenance_loop();
    DaemonConfig config_;
    AppState state_;
    UniqueSocket listener_;
    ConnectionManager connections_;
    bool shutting_down_;
#ifdef _WIN32
    HANDLE accept_thread_;
#else
    std::thread* accept_thread_;
#endif
};
```

```cpp
// include/server_transport.h
void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms);

// src/server_transport.cpp
void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms) {
#ifdef _WIN32
    const DWORD value = static_cast<DWORD>(timeout_ms);
    setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&value), sizeof(value));
    setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, reinterpret_cast<const char*>(&value), sizeof(value));
#else
    struct timeval value;
    value.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    value.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, &value, sizeof(value));
    setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, &value, sizeof(value));
#endif
}
```

```cpp
// src/http_connection.cpp
static const unsigned long HTTP_CONNECTION_IDLE_TIMEOUT_MS = 30000UL;

void handle_client(AppState& state, UniqueSocket client) {
    set_socket_timeout_ms(client.get(), HTTP_CONNECTION_IDLE_TIMEOUT_MS);
    for (;;) {
        // existing read / route / send loop stays intact
    }
}
```

```cpp
// src/server.cpp
int run_server(const DaemonConfig& config) {
    NetworkSession network;
    ServerRuntime runtime(config);
    runtime.start_accept_loop();
    runtime.join();
    return 0;
}
```

- [ ] **Step 4: Run the runtime and streaming HTTP verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
Expected: PASS with orderly startup, shutdown, and active-worker cleanup.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS with persistent requests and streaming transfer behavior preserved under supervised workers.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/server_runtime.h \
        crates/remote-exec-daemon-cpp/src/server_runtime.cpp \
        crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp \
        crates/remote-exec-daemon-cpp/include/server.h \
        crates/remote-exec-daemon-cpp/include/server_transport.h \
        crates/remote-exec-daemon-cpp/src/server_transport.cpp \
        crates/remote-exec-daemon-cpp/src/server.cpp \
        crates/remote-exec-daemon-cpp/src/http_connection.cpp \
        crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp \
        crates/remote-exec-daemon-cpp/Makefile
git commit -m "refactor: supervise cpp http server runtime"
```

### Task 5: Unify Port-Forward Close Paths And Resource States

**Files:**
- Create: `crates/remote-exec-daemon-cpp/tests/test_port_forward.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/port_forward.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-port-forward`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `TDD`
Reason: port-forward close semantics are directly observable at the store level and can fail fast before the route layer is involved.

- [ ] **Step 1: Add the failing direct store-level tests for close-during-accept and close-during-read**

```cpp
// tests/test_port_forward.cpp
#include <cassert>
#include <thread>

#include "port_forward.h"
#include "platform.h"

int main() {
    PortForwardStore store;
    const Json listener = store.listen("127.0.0.1:0", "tcp", "", 0);
    const std::string bind_id = listener.at("bind_id").get<std::string>();

    bool closed_during_accept = false;
    std::thread accept_thread([&]() {
        try {
            (void)store.listen_accept(bind_id);
        } catch (const PortForwardError& ex) {
            closed_during_accept = ex.code() == "port_bind_closed";
        }
    });
    platform::sleep_ms(50);
    (void)store.listen_close(bind_id);
    accept_thread.join();
    assert(closed_during_accept);
}
```

```make
# Makefile additions
HOST_PORT_FORWARD := $(BUILD_DIR)/test_port_forward
HOST_PORT_FORWARD_SRCS := $(MAKEFILE_DIR)tests/test_port_forward.cpp $(PORT_FORWARD_SRCS) $(addprefix $(MAKEFILE_DIR),src/server_transport.cpp src/http_helpers.cpp src/http_codec.cpp src/http_request.cpp src/text_utils.cpp src/basic_mutex.cpp src/platform.cpp src/logging.cpp)
HOST_PORT_FORWARD_OBJS := $(sort $(call host_test_objs,$(HOST_PORT_FORWARD_SRCS)))
test-host-port-forward: $(HOST_PORT_FORWARD)
	$(HOST_PORT_FORWARD)

$(HOST_PORT_FORWARD): $(HOST_PORT_FORWARD_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^
```

- [ ] **Step 2: Run the new port-forward target before unifying the close paths**

Run: `make -C crates/remote-exec-daemon-cpp test-host-port-forward`
Expected: FAIL until the direct test target exists and the close-path semantics are made deterministic.

- [ ] **Step 3: Make listener and TCP connection closure idempotent and state-driven**

```cpp
// include/port_forward.h
enum PortResourceState {
    PORT_RESOURCE_OPEN = 0,
    PORT_RESOURCE_CLOSING = 1,
    PORT_RESOURCE_CLOSED = 2
};

struct TcpConnection {
    TcpConnection(SOCKET socket, const std::string& lease_id);
    UniqueSocket socket;
    BasicMutex state_mutex;
    BasicMutex read_mutex;
    BasicMutex write_mutex;
    PortResourceState state;
    std::string lease_id;
};
```

```cpp
// src/port_forward.cpp
static bool begin_close_tcp_connection(const std::shared_ptr<TcpConnection>& connection) {
    BasicLockGuard lock(connection->state_mutex);
    if (connection->state != PORT_RESOURCE_OPEN) {
        return false;
    }
    connection->state = PORT_RESOURCE_CLOSING;
    return true;
}

Json PortForwardStore::listen_close(const std::string& bind_id) {
    std::shared_ptr<SharedSocket> listener;
    {
        BasicLockGuard lock(mutex_);
        std::map<std::string, std::shared_ptr<SharedSocket> >::iterator it = tcp_listeners_.find(bind_id);
        if (it != tcp_listeners_.end()) {
            listener = it->second;
            tcp_listeners_.erase(it);
        }
    }
    if (listener.get() != NULL) {
        mark_shared_socket_closed(listener);
        if (!listener->lease_id.empty()) {
            untrack_bind_lease(listener->lease_id, bind_id);
        }
    }
    return Json::object();
}
```

- [ ] **Step 4: Run direct port-forward coverage and route-level regression coverage**

Run: `make -C crates/remote-exec-daemon-cpp test-host-port-forward`
Expected: PASS with deterministic close-during-blocked-operation behavior.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS with the public route surface unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/tests/test_port_forward.cpp \
        crates/remote-exec-daemon-cpp/include/port_forward.h \
        crates/remote-exec-daemon-cpp/src/port_forward.cpp \
        crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
        crates/remote-exec-daemon-cpp/Makefile
git commit -m "refactor: unify cpp port forward lifecycle"
```

### Task 6: Add Background Maintenance And Fold The New Targets Into The Quality Gate

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/port_forward.h`
- Modify: `crates/remote-exec-daemon-cpp/include/server_runtime.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-wine-session-store`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** `existing tests + targeted verification`
Reason: maintenance is orchestration logic layered on top of the already-tested session, connection, and port-forward units, so the right proof is one focused runtime test plus the full crate quality gate.

- [ ] **Step 1: Extend the runtime test to require maintenance-driven reaping and sweep the new host targets into `check-posix`**

```cpp
// tests/test_server_runtime.cpp
runtime.connection_manager().reap_finished();
runtime.maintenance_once();
assert(runtime.connection_manager().active_count() == 0UL);
```

```make
# Makefile additions
check-posix: test-host-basic-mutex test-host-patch test-host-transfer test-host-config test-host-http-request test-host-server-transport test-host-server-streaming test-host-session-store test-host-server-routes test-host-connection-manager test-host-server-runtime test-host-port-forward test-host-sandbox all-posix

.PHONY: test-host-basic-mutex test-host-connection-manager test-host-server-runtime test-host-port-forward
```

- [ ] **Step 2: Run the focused runtime coverage before adding the maintenance loop**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
Expected: FAIL until `maintenance_once()` and the background maintenance thread exist.

- [ ] **Step 3: Add the maintenance loop and one-shot sweep helper to `ServerRuntime`**

```cpp
// include/port_forward.h
class PortForwardStore {
public:
    void sweep_expired_leases_for_runtime();
};

// include/server_runtime.h
void maintenance_once();

// src/server_runtime.cpp
void ServerRuntime::maintenance_once() {
    connections_.reap_finished();
    state_.port_forwards.sweep_expired_leases_for_runtime();
}

void ServerRuntime::maintenance_loop() {
    while (!shutting_down_) {
        maintenance_once();
        platform::sleep_ms(250);
    }
}
```

- [ ] **Step 4: Run the full verification stack with the new targets included**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
Expected: PASS with explicit maintenance coverage.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS across the expanded POSIX host test set and production build.

Run: `make -C crates/remote-exec-daemon-cpp test-wine-session-store`
Expected: PASS with the XP-compatible session path still working after the event-driven session refactor.

Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
Expected: PASS with the XP-compatible production build still clean.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/server_runtime.h \
        crates/remote-exec-daemon-cpp/src/server_runtime.cpp \
        crates/remote-exec-daemon-cpp/src/connection_manager.cpp \
        crates/remote-exec-daemon-cpp/src/port_forward.cpp \
        crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp \
        crates/remote-exec-daemon-cpp/Makefile
git commit -m "refactor: add cpp maintenance supervision"
```
