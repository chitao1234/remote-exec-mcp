#ifndef _WIN32

#include "exec/posix_child_reaper.h"

#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <map>
#include <set>
#include <stdexcept>
#include <thread>
#include <vector>

#include <poll.h>
#include <sys/select.h>
#include <sys/wait.h>
#include <unistd.h>

#include "core/logging.h"
#include "platform/basic_mutex.h"
#include "platform/platform.h"
#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"
#include "platform/posix_process.h"
#include "platform/posix_signal.h"
#include "runtime/daemon_thread.h"

namespace {

BasicMutex g_mutex;
std::set<pid_t> g_registered;
std::map<pid_t, int> g_reaped;
int g_signal_pipe_read = -1;
int g_signal_pipe_write = -1;
bool g_installed = false;
std::atomic<bool> g_stopping(false);
std::unique_ptr<std::thread> g_reaper_thread;

#ifdef REMOTE_EXEC_CPP_TESTING
std::atomic<unsigned long> g_test_reap_delay_ms(0UL);
#endif

void sigchld_handler(int) {
    if (g_signal_pipe_write >= 0) {
        posix_fd::write_signal_safe_wakeup_byte(g_signal_pipe_write);
    }
}

bool take_reaped_locked(pid_t pid, int* status) {
    std::map<pid_t, int>::iterator it = g_reaped.find(pid);
    if (it == g_reaped.end()) {
        return false;
    }
    *status = it->second;
    g_reaped.erase(it);
    g_registered.erase(pid);
    return true;
}

void record_reaped(pid_t pid, int status) {
    if (g_registered.find(pid) != g_registered.end()) {
        g_reaped[pid] = status;
    }
}

void reap_registered_children() {
    BasicLockGuard lock(g_mutex);
    const std::vector<pid_t> pids(g_registered.begin(), g_registered.end());
    for (std::size_t i = 0; i < pids.size(); ++i) {
        if (g_reaped.find(pids[i]) != g_reaped.end()) {
            continue;
        }
        int status = 0;
        const pid_t result = posix_process::wait_pid(pids[i], &status, WNOHANG);
        if (result == pids[i]) {
#ifdef REMOTE_EXEC_CPP_TESTING
            const unsigned long delay_ms = g_test_reap_delay_ms.load();
            if (delay_ms > 0UL) {
                std::this_thread::sleep_for(std::chrono::milliseconds(delay_ms));
            }
#endif
            record_reaped(pids[i], status);
        }
    }
}

void drain_signal_pipe() {
    unsigned char buffer[64];
    while (g_signal_pipe_read >= 0
           && posix_fd::read_retry(g_signal_pipe_read, buffer, sizeof(buffer)) > 0) {
    }
}

void reaper_loop() {
    while (!g_stopping.load(std::memory_order_relaxed)) {
        struct pollfd descriptor;
        descriptor.fd = g_signal_pipe_read;
        descriptor.events = POLLIN;
        descriptor.revents = 0;
        const int ready = posix_eintr::poll_for_ms(&descriptor, 1, 1000UL);
        if (ready > 0) {
            drain_signal_pipe();
        }
        reap_registered_children();
    }
}

} // namespace

void install_posix_child_reaper() {
    BasicLockGuard lock(g_mutex);
    if (g_installed) {
        return;
    }
    int fds[2];
    if (posix_fd::create_cloexec_pipe(fds) != 0) {
        throw std::runtime_error(errno_error::operation_failed("pipe(SIGCHLD)", errno));
    }
    g_signal_pipe_read = fds[0];
    g_signal_pipe_write = fds[1];
    (void)posix_fd::set_nonblocking(g_signal_pipe_read);
    (void)posix_fd::set_nonblocking(g_signal_pipe_write);

    if (posix_signal::install_handler(SIGCHLD, sigchld_handler, SA_RESTART | SA_NOCLDSTOP) != 0) {
        throw std::runtime_error(errno_error::operation_failed("sigaction(SIGCHLD)", errno));
    }

    g_reaper_thread.reset(new std::thread(reaper_loop));
    g_installed = true;
    // The reaper is a process-lifetime service; tests and embedded uses may
    // install it without ever calling shutdown_posix_child_reaper(). Join it
    // at exit so a joinable std::thread is not destroyed after main.
    (void)std::atexit(shutdown_posix_child_reaper);
    log_message(LOG_INFO, "posix_child_reaper", "installed SIGCHLD child reaper");
}

void shutdown_posix_child_reaper() {
    if (!g_installed) {
        return;
    }
    g_stopping.store(true, std::memory_order_relaxed);
    if (g_signal_pipe_write >= 0) {
        const char wakeup_byte = 1;
        ssize_t ignored = posix_fd::write_retry(g_signal_pipe_write, &wakeup_byte, 1);
        (void)ignored;
    }
    consume_daemon_thread(&g_reaper_thread);
}

void register_posix_child(pid_t pid) {
    BasicLockGuard lock(g_mutex);
    g_registered.insert(pid);
}

bool wait_posix_child_exit_impl(
    pid_t pid,
    int* status,
    int wait_flags,
    const char* echild_message,
    bool throw_on_other_error
) {
    BasicLockGuard lock(g_mutex);
    if (take_reaped_locked(pid, status)) {
        return true;
    }

    const pid_t result = posix_process::wait_pid(pid, status, wait_flags);
    if (result == pid) {
        g_registered.erase(pid);
        g_reaped.erase(pid);
        return true;
    }
    if (result == 0) {
        return false;
    }

    if (result < 0 && errno == ECHILD) {
        if (take_reaped_locked(pid, status)) {
            return true;
        }
        g_registered.erase(pid);
        log_message(LOG_WARN, "posix_child_reaper", echild_message);
        *status = 0;
        return true;
    }

    if (throw_on_other_error) {
        throw std::runtime_error(errno_error::operation_failed("waitpid", errno));
    }
    return false;
}

bool poll_posix_child_exit(pid_t pid, int* status) {
    return wait_posix_child_exit_impl(
        pid,
        status,
        WNOHANG,
        "lost child status after ECHILD; assuming zero exit status",
        true
    );
}

bool wait_posix_child_exit(pid_t pid, int* status) {
    return wait_posix_child_exit_impl(
        pid,
        status,
        0,
        "lost child status during blocking wait; assuming zero exit status",
        false
    );
}

#ifdef REMOTE_EXEC_CPP_TESTING
void set_posix_child_reaper_test_reap_delay_ms(unsigned long delay_ms) {
    g_test_reap_delay_ms.store(delay_ms);
}
#endif

#endif
