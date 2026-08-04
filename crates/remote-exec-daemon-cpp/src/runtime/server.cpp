#include <cstdio>
#include <stdexcept>
#include <string>

#ifndef _WIN32
#include <csignal>
#include <poll.h>
#include <unistd.h>
#endif

#include "core/logging.h"
#include "platform/path_utils.h"
#include "platform/platform.h"
#ifndef _WIN32
#include "exec/posix_child_reaper.h"
#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"
#include "platform/posix_signal.h"
#endif
#include "core/stdio_retry.h"
#include "platform/scoped_file.h"
#include "runtime/server.h"
#include "runtime/server_runtime.h"

#ifndef _WIN32
namespace {

int g_shutdown_pipe_read = -1;
int g_shutdown_pipe_write = -1;
volatile sig_atomic_t g_shutdown_requested = 0;

void shutdown_signal_handler(int) {
    g_shutdown_requested = 1;
    if (g_shutdown_pipe_write >= 0) {
        posix_fd::write_signal_safe_wakeup_byte(g_shutdown_pipe_write);
    }
}

bool install_shutdown_signal_handlers() {
    int fds[2];
    if (posix_fd::create_cloexec_pipe(fds) != 0) {
        return false;
    }
    g_shutdown_pipe_read = fds[0];
    g_shutdown_pipe_write = fds[1];
    if (!posix_fd::set_nonblocking(fds[0]) || !posix_fd::set_nonblocking(fds[1])) {
        posix_fd::close_ignoring_errors(fds[0]);
        posix_fd::close_ignoring_errors(fds[1]);
        g_shutdown_pipe_read = -1;
        g_shutdown_pipe_write = -1;
        return false;
    }

    if (posix_signal::install_handler(SIGTERM, shutdown_signal_handler, 0) != 0
        || posix_signal::install_handler(SIGINT, shutdown_signal_handler, 0) != 0) {
        posix_fd::close_ignoring_errors(fds[0]);
        posix_fd::close_ignoring_errors(fds[1]);
        g_shutdown_pipe_read = -1;
        g_shutdown_pipe_write = -1;
        return false;
    }
    return true;
}

void wait_for_shutdown_signal() {
    struct pollfd pfd;
    pfd.fd = g_shutdown_pipe_read;
    pfd.events = POLLIN;
    pfd.revents = 0;
    if (!g_shutdown_requested) {
        (void)posix_eintr::poll_forever_until(&pfd, 1, []() { return g_shutdown_requested != 0; });
    }
}

} // namespace
#endif

#ifdef _WIN32
namespace {
ServerRuntime* g_runtime_for_shutdown = nullptr;

BOOL WINAPI console_ctrl_handler(DWORD ctrl_type) {
    if (ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT
        || ctrl_type == CTRL_CLOSE_EVENT) {
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
    if (!stdio_retry::fwrite_all(out.get(), line.data(), line.size()) || out.close() != 0) {
        throw std::runtime_error("failed to write test_bound_addr_file");
    }
}

int run_server(const DaemonConfig& config) {
    NetworkSession network;
    {
        ServerRuntime runtime(config);
#ifndef _WIN32
        install_posix_child_reaper();
        install_shutdown_signal_handlers();
#else
        g_runtime_for_shutdown = &runtime;
        SetConsoleCtrlHandler(console_ctrl_handler, TRUE);
#endif
        if (config.connection_mode == ConnectionMode::Reverse) {
            runtime.start_reverse_loop();
        } else {
            runtime.start_accept_loop();
            const unsigned short bound_port = runtime.bound_port();
            write_test_bound_addr_file(runtime.config(), bound_port);
        }

        LogMessageBuilder message(
            config.connection_mode == ConnectionMode::Reverse ? "connecting" : "listening"
        );
        message.raw("on")
            .raw(runtime.config().listen_host)
            .field(
                "port",
                config.connection_mode == ConnectionMode::Reverse ? config.reverse_broker_port
                                                                  : runtime.bound_port()
            )
            .quoted_field("target", runtime.config().target)
            .bool_field("http_auth_enabled", !runtime.config().http_auth_bearer_token.empty())
            .quoted_field("platform", platform::platform_name())
            .quoted_field("arch", platform::arch_name())
            .quoted_field("default_shell", runtime.metadata().default_shell)
            .quoted_field("daemon_instance_id", runtime.metadata().daemon_instance_id);
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
    }
#ifndef _WIN32
    shutdown_posix_child_reaper();
#endif
    log_message(LOG_INFO, "server", "shutdown complete");
    return 0;
}
