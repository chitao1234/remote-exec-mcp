#ifndef _WIN32

#include "posix_child_reaper.h"

#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
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

#include "platform/basic_mutex.h"
#include "core/logging.h"
#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"

namespace {

BasicMutex g_mutex;
std::set<pid_t> g_registered;
std::map<pid_t, int> g_reaped;
int g_signal_pipe_read = -1;
int g_signal_pipe_write = -1;
bool g_installed = false;
std::atomic<bool> g_stopping(false);
std::thread* g_reaper_thread = nullptr;

#ifdef REMOTE_EXEC_CPP_TESTING
std::atomic<unsigned long> g_test_reap_delay_ms(0UL);
#endif

void sigchld_handler(int) {
    if (g_signal_pipe_write >= 0) {
        const unsigned char byte = 1U;
        ssize_t ignored = write(g_signal_pipe_write, &byte, 1U);
        (void)ignored;
    }
}

pid_t waitpid_retry_on_eintr(pid_t pid, int* status, int options) {
    return posix_eintr::retry<pid_t>([&]() { return waitpid(pid, status, options); });
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
        for (;;) {
            const pid_t result = waitpid_retry_on_eintr(pids[i], &status, WNOHANG);
            if (result == pids[i]) {
#ifdef REMOTE_EXEC_CPP_TESTING
                const unsigned long delay_ms = g_test_reap_delay_ms.load();
                if (delay_ms > 0UL) {
                    std::this_thread::sleep_for(std::chrono::milliseconds(delay_ms));
                }
#endif
                record_reaped(pids[i], status);
                break;
            }
            if (result == 0) {
                break;
            }
            break;
        }
    }
}

void drain_signal_pipe() {
    unsigned char buffer[64];
    while (g_signal_pipe_read >= 0 &&
           posix_eintr::retry<ssize_t>([&]() { return read(g_signal_pipe_read, buffer, sizeof(buffer)); }) > 0) {
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
    if (posix_eintr::retry<int>([&]() { return pipe(fds); }) != 0) {
        throw std::runtime_error(std::string("pipe(SIGCHLD) failed: ") + std::strerror(errno));
    }
    g_signal_pipe_read = fds[0];
    g_signal_pipe_write = fds[1];
    (void)posix_fd::set_cloexec_nonblocking(g_signal_pipe_read);
    (void)posix_fd::set_cloexec_nonblocking(g_signal_pipe_write);

    struct sigaction action;
    std::memset(&action, 0, sizeof(action));
    action.sa_handler = sigchld_handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART | SA_NOCLDSTOP;
    if (sigaction(SIGCHLD, &action, nullptr) != 0) {
        throw std::runtime_error(std::string("sigaction(SIGCHLD) failed: ") + std::strerror(errno));
    }

    g_reaper_thread = new std::thread(reaper_loop);
    g_installed = true;
    log_message(LOG_INFO, "posix_child_reaper", "installed SIGCHLD child reaper");
}

void shutdown_posix_child_reaper() {
    if (!g_installed) {
        return;
    }
    g_stopping.store(true, std::memory_order_relaxed);
    if (g_signal_pipe_write >= 0) {
        const char byte = 1;
        ssize_t ignored = posix_eintr::retry<ssize_t>([&]() { return write(g_signal_pipe_write, &byte, 1); });
        (void)ignored;
    }
    if (g_reaper_thread != nullptr) {
        g_reaper_thread->join();
        delete g_reaper_thread;
        g_reaper_thread = nullptr;
    }
}

void register_posix_child(pid_t pid) {
    BasicLockGuard lock(g_mutex);
    g_registered.insert(pid);
}

void unregister_posix_child(pid_t pid) {
    BasicLockGuard lock(g_mutex);
    g_registered.erase(pid);
    g_reaped.erase(pid);
}

bool take_reaped_posix_child(pid_t pid, int* status) {
    BasicLockGuard lock(g_mutex);
    return take_reaped_locked(pid, status);
}

bool poll_posix_child_exit(pid_t pid, int* status) {
    BasicLockGuard lock(g_mutex);
    if (take_reaped_locked(pid, status)) {
        return true;
    }

    const pid_t result = waitpid_retry_on_eintr(pid, status, WNOHANG);
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
        log_message(LOG_WARN, "posix_child_reaper", "lost child status after ECHILD; assuming zero exit status");
        *status = 0;
        return true;
    }

    throw std::runtime_error(std::string("waitpid failed: ") + std::strerror(errno));
}

bool wait_posix_child_exit(pid_t pid, int* status) {
    BasicLockGuard lock(g_mutex);
    if (take_reaped_locked(pid, status)) {
        return true;
    }

    const pid_t result = waitpid_retry_on_eintr(pid, status, 0);
    if (result == pid) {
        g_registered.erase(pid);
        g_reaped.erase(pid);
        return true;
    }

    if (result < 0 && errno == ECHILD) {
        if (take_reaped_locked(pid, status)) {
            return true;
        }
        g_registered.erase(pid);
        log_message(
            LOG_WARN, "posix_child_reaper", "lost child status during blocking wait; assuming zero exit status");
        *status = 0;
        return true;
    }

    return false;
}

#ifdef REMOTE_EXEC_CPP_TESTING
void set_posix_child_reaper_test_reap_delay_ms(unsigned long delay_ms) {
    g_test_reap_delay_ms.store(delay_ms);
}
#endif

#endif
