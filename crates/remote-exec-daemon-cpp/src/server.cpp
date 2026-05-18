#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <string>

#ifndef _WIN32
#include <cerrno>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <unistd.h>
#endif

#include "logging.h"
#include "path_utils.h"
#include "platform.h"
#ifndef _WIN32
#include "posix_child_reaper.h"
#endif
#include "scoped_file.h"
#include "server.h"
#include "server_runtime.h"

#ifndef _WIN32
namespace {

int g_shutdown_pipe_read = -1;
int g_shutdown_pipe_write = -1;

void shutdown_signal_handler(int) {
    const char byte = 1;
    ssize_t ignored = write(g_shutdown_pipe_write, &byte, 1);
    (void)ignored;
}

bool install_shutdown_signal_handlers() {
    int fds[2];
    if (pipe(fds) != 0) {
        return false;
    }
    g_shutdown_pipe_read = fds[0];
    g_shutdown_pipe_write = fds[1];
    fcntl(fds[0], F_SETFD, fcntl(fds[0], F_GETFD) | FD_CLOEXEC);
    fcntl(fds[0], F_SETFL, fcntl(fds[0], F_GETFL) | O_NONBLOCK);
    fcntl(fds[1], F_SETFD, fcntl(fds[1], F_GETFD) | FD_CLOEXEC);
    fcntl(fds[1], F_SETFL, fcntl(fds[1], F_GETFL) | O_NONBLOCK);

    struct sigaction sa;
    std::memset(&sa, 0, sizeof(sa));
    sa.sa_handler = shutdown_signal_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    sigaction(SIGTERM, &sa, nullptr);
    sigaction(SIGINT, &sa, nullptr);
    return true;
}

void wait_for_shutdown_signal() {
    struct pollfd pfd;
    pfd.fd = g_shutdown_pipe_read;
    pfd.events = POLLIN;
    pfd.revents = 0;
    for (;;) {
        const int result = poll(&pfd, 1, -1);
        if (result > 0) {
            return;
        }
        if (result < 0 && errno != EINTR) {
            return;
        }
    }
}

} // namespace
#endif

#ifdef _WIN32
namespace {
ServerRuntime* g_runtime_for_shutdown = nullptr;

BOOL WINAPI console_ctrl_handler(DWORD ctrl_type) {
    if (ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT || ctrl_type == CTRL_CLOSE_EVENT) {
        ServerRuntime* runtime = g_runtime_for_shutdown;
        if (runtime != nullptr) {
            runtime->request_shutdown();
        }
        return TRUE;
    }
    return FALSE;
}
} // namespace
#endif

static void write_test_bound_addr_file(const DaemonConfig& config, unsigned short bound_port) {
    if (config.test_bound_addr_file.empty()) {
        return;
    }
    ScopedFile out(path_utils::open_file(config.test_bound_addr_file, "wb"));
    if (!out.valid()) {
        throw std::runtime_error("failed to open test_bound_addr_file");
    }
    const std::string line = config.listen_host + ":" + std::to_string(bound_port) + "\n";
    if (std::fwrite(line.data(), 1, line.size(), out.get()) != line.size() || out.close() != 0) {
        throw std::runtime_error("failed to write test_bound_addr_file");
    }
}

int run_server(const DaemonConfig& config) {
    NetworkSession network;
    ServerRuntime runtime(config);
#ifndef _WIN32
    install_posix_child_reaper();
    install_shutdown_signal_handlers();
#else
    g_runtime_for_shutdown = &runtime;
    SetConsoleCtrlHandler(console_ctrl_handler, TRUE);
#endif
    runtime.start_accept_loop();
    const unsigned short bound_port = runtime.bound_port();
    write_test_bound_addr_file(runtime.state().config, bound_port);

    LogMessageBuilder message("listening");
    message.raw("on")
        .raw(runtime.state().config.listen_host)
        .field("port", bound_port)
        .quoted_field("target", runtime.state().config.target)
        .bool_field("http_auth_enabled", !runtime.state().config.http_auth_bearer_token.empty())
        .quoted_field("platform", platform::platform_name())
        .quoted_field("arch", platform::arch_name())
        .quoted_field("default_shell", runtime.state().default_shell)
        .quoted_field("daemon_instance_id", runtime.state().daemon_instance_id);
    log_message(LOG_INFO, "server", message.str());

#ifndef _WIN32
    wait_for_shutdown_signal();
    log_message(LOG_INFO, "server", "shutdown signal received");
    runtime.request_shutdown();
#endif
    runtime.join();
#ifdef _WIN32
    g_runtime_for_shutdown = nullptr;
#endif
    log_message(LOG_INFO, "server", "shutdown complete");
    return 0;
}
