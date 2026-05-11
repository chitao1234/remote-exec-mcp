#ifndef _WIN32

#include "posix_child_reaper.h"

#include <cerrno>
#include <csignal>
#include <cstring>
#include <map>
#include <set>
#include <stdexcept>
#include <thread>
#include <vector>

#include <fcntl.h>
#include <sys/select.h>
#include <sys/wait.h>
#include <unistd.h>

#include "basic_mutex.h"
#include "logging.h"

namespace {

BasicMutex g_mutex;
std::set<pid_t> g_registered;
std::map<pid_t, int> g_reaped;
int g_signal_pipe_read = -1;
int g_signal_pipe_write = -1;
bool g_installed = false;

void set_cloexec_nonblock(int fd) {
    const int fd_flags = fcntl(fd, F_GETFD, 0);
    if (fd_flags >= 0) {
        fcntl(fd, F_SETFD, fd_flags | FD_CLOEXEC);
    }
    const int status_flags = fcntl(fd, F_GETFL, 0);
    if (status_flags >= 0) {
        fcntl(fd, F_SETFL, status_flags | O_NONBLOCK);
    }
}

void sigchld_handler(int) {
    if (g_signal_pipe_write >= 0) {
        const unsigned char byte = 1U;
        (void)write(g_signal_pipe_write, &byte, 1U);
    }
}

std::vector<pid_t> registered_snapshot() {
    BasicLockGuard lock(g_mutex);
    return std::vector<pid_t>(g_registered.begin(), g_registered.end());
}

void record_reaped(pid_t pid, int status) {
    BasicLockGuard lock(g_mutex);
    if (g_registered.find(pid) != g_registered.end()) {
        g_reaped[pid] = status;
    }
}

void reap_registered_children() {
    const std::vector<pid_t> pids = registered_snapshot();
    for (std::size_t i = 0; i < pids.size(); ++i) {
        int status = 0;
        for (;;) {
            const pid_t result = waitpid(pids[i], &status, WNOHANG);
            if (result == pids[i]) {
                record_reaped(pids[i], status);
                break;
            }
            if (result == 0) {
                break;
            }
            if (result < 0 && errno == EINTR) {
                continue;
            }
            break;
        }
    }
}

void drain_signal_pipe() {
    unsigned char buffer[64];
    while (g_signal_pipe_read >= 0 && read(g_signal_pipe_read, buffer, sizeof(buffer)) > 0) {
    }
}

void reaper_loop() {
    for (;;) {
        fd_set read_fds;
        FD_ZERO(&read_fds);
        FD_SET(g_signal_pipe_read, &read_fds);
        timeval timeout;
        timeout.tv_sec = 1;
        timeout.tv_usec = 0;
        const int ready = select(g_signal_pipe_read + 1, &read_fds, NULL, NULL, &timeout);
        if (ready > 0) {
            drain_signal_pipe();
        }
        reap_registered_children();
    }
}

}  // namespace

void install_posix_child_reaper() {
    BasicLockGuard lock(g_mutex);
    if (g_installed) {
        return;
    }
    int fds[2];
    if (pipe(fds) != 0) {
        throw std::runtime_error(std::string("pipe(SIGCHLD) failed: ") + std::strerror(errno));
    }
    g_signal_pipe_read = fds[0];
    g_signal_pipe_write = fds[1];
    set_cloexec_nonblock(g_signal_pipe_read);
    set_cloexec_nonblock(g_signal_pipe_write);

    struct sigaction action;
    std::memset(&action, 0, sizeof(action));
    action.sa_handler = sigchld_handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART | SA_NOCLDSTOP;
    if (sigaction(SIGCHLD, &action, NULL) != 0) {
        throw std::runtime_error(std::string("sigaction(SIGCHLD) failed: ") + std::strerror(errno));
    }

    std::thread(reaper_loop).detach();
    g_installed = true;
    log_message(LOG_INFO, "posix_child_reaper", "installed SIGCHLD child reaper");
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
    std::map<pid_t, int>::iterator it = g_reaped.find(pid);
    if (it == g_reaped.end()) {
        return false;
    }
    *status = it->second;
    g_reaped.erase(it);
    g_registered.erase(pid);
    return true;
}

#endif
