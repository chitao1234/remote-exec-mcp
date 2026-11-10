#include <cstdio>
#include <stdexcept>
#include <string>
#include <thread>

#ifndef _WIN32
#include <csignal>
#include <poll.h>
#include <unistd.h>
#endif

#include "core/logging.h"
#include "core/stdio_retry.h"
#include "platform/path_utils.h"
#include "platform/platform.h"
#include "platform/scoped_file.h"
#include "runtime/reactor_server.h"
#include "runtime/server.h"

#ifndef _WIN32
#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"
#include "platform/posix_signal.h"
#endif

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

void wake_shutdown_waiter() {
    shutdown_signal_handler(0);
}

} // namespace
#else
namespace {

struct ConsoleShutdownState {
    ConsoleShutdownState()
        : requested(CreateEvent(nullptr, TRUE, FALSE, nullptr)),
          completed(CreateEvent(nullptr, TRUE, FALSE, nullptr)) {
        if (requested == nullptr || completed == nullptr) {
            if (requested != nullptr) {
                CloseHandle(requested);
            }
            if (completed != nullptr) {
                CloseHandle(completed);
            }
            throw std::runtime_error("failed to create console shutdown events");
        }
    }

    const HANDLE requested;
    const HANDLE completed;
};

ConsoleShutdownState& console_shutdown_state() {
    // Console callbacks may still be running after the handler is unregistered.
    // Keep only these events for process lifetime; callbacks never own or touch
    // the runtime. Windows reclaims the handles when the process exits.
    static ConsoleShutdownState* const state = new ConsoleShutdownState();
    return *state;
}

#ifdef REMOTE_EXEC_CPP_TESTING
std::atomic<bool> g_console_shutdown_registered(false);
#endif

BOOL WINAPI console_ctrl_handler(DWORD ctrl_type) {
    if (ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT
        || ctrl_type == CTRL_CLOSE_EVENT) {
        ConsoleShutdownState& state = console_shutdown_state();
        SetEvent(state.requested);
        // CTRL_CLOSE_EVENT terminates the process after this callback returns.
        // Allow the main thread to finish shutting down before acknowledging it.
        WaitForSingleObject(state.completed, INFINITE);
        return TRUE;
    }
    return FALSE;
}

class ConsoleShutdownRegistration {
public:
    ConsoleShutdownRegistration() {
        ConsoleShutdownState& state = console_shutdown_state();
        ResetEvent(state.requested);
        ResetEvent(state.completed);
        if (!SetConsoleCtrlHandler(console_ctrl_handler, TRUE)) {
            throw std::runtime_error("failed to register console shutdown handler");
        }
#ifdef REMOTE_EXEC_CPP_TESTING
        g_console_shutdown_registered.store(true);
#endif
    }

    ~ConsoleShutdownRegistration() {
        SetEvent(console_shutdown_state().completed);
        SetConsoleCtrlHandler(console_ctrl_handler, FALSE);
#ifdef REMOTE_EXEC_CPP_TESTING
        g_console_shutdown_registered.store(false);
#endif
    }
};

void wait_for_shutdown_signal() {
    WaitForSingleObject(console_shutdown_state().requested, INFINITE);
}

void wake_shutdown_waiter() {
    SetEvent(console_shutdown_state().requested);
}

} // namespace

#ifdef REMOTE_EXEC_CPP_TESTING
bool console_shutdown_registered_for_testing() {
    return g_console_shutdown_registered.load();
}

bool invoke_console_shutdown_handler_for_testing(unsigned long ctrl_type) {
    return console_ctrl_handler(ctrl_type) != FALSE;
}
#endif
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
#ifdef _WIN32
    ConsoleShutdownRegistration shutdown_registration;
#else
    install_shutdown_signal_handlers();
#endif

    ReactorServer server(config);
    std::string start_error;
    if (!server.start(&start_error)) {
        log_message(LOG_ERROR, "server", start_error);
        std::fprintf(stderr, "%s\n", start_error.c_str());
        return 1;
    }

    if (config.connection_mode == ConnectionMode::Listen) {
        write_test_bound_addr_file(config, server.bound_port());
    }

    LogMessageBuilder message(
        config.connection_mode == ConnectionMode::Reverse ? "connecting" : "listening"
    );
    const std::string log_host = config.connection_mode == ConnectionMode::Reverse
                                     ? config.reverse_broker_host
                                     : config.listen_host;
    const int log_port = config.connection_mode == ConnectionMode::Reverse
                             ? config.reverse_broker_port
                             : static_cast<int>(server.bound_port());
    message.raw("on")
        .raw(log_host)
        .field("port", log_port)
        .quoted_field("target", server.config().target)
        .bool_field("http_auth_enabled", !server.config().http_auth_bearer_token.empty())
        .quoted_field("platform", platform::platform_name())
        .quoted_field("arch", platform::arch_name())
        .quoted_field("default_shell", server.metadata().default_shell)
        .quoted_field("daemon_instance_id", server.metadata().daemon_instance_id);
    log_message(LOG_INFO, "server", message.str());

    std::thread shutdown_thread([&server]() {
        wait_for_shutdown_signal();
        server.request_stop();
    });

    server.run();
    wake_shutdown_waiter();
    shutdown_thread.join();

    log_message(LOG_INFO, "server", "shutdown complete");
    return 0;
}
