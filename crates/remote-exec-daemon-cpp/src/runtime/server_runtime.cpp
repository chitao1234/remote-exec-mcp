#include "runtime/server_runtime.h"

#include <cerrno>
#include <cstring>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <thread>

#include "capabilities/daemon_capabilities.h"
#include "core/logging.h"
#include "http/server_transport.h"
#include "policy/path_policy.h"
#include "platform/platform.h"
#include "port_forward/port_forward_socket_ops.h"
#include "port_forward/port_tunnel.h"
#include "runtime/daemon_thread.h"
#include "../port_forward/port_tunnel_service.h"

namespace {

std::string daemon_instance_id() {
    std::ostringstream out;
    out << platform::monotonic_ms();
    return out.str();
}

} // namespace

ServerRuntime::ServerRuntime(const DaemonConfig& config)
    : connections_(config.max_open_sessions), shutting_down_(false), accept_thread_(), maintenance_thread_()
{
    state_.config = config;
    state_.daemon_instance_id = daemon_instance_id();
    state_.hostname = platform::hostname();
    state_.default_shell = platform::resolve_default_shell(config.default_shell);
    state_.capabilities = detect_daemon_capabilities();
    state_.sandbox_enabled = config.sandbox_configured;
    if (state_.sandbox_enabled) {
        state_.sandbox = compile_filesystem_sandbox(config.sandbox);
    }
    state_.port_tunnel_service = create_port_tunnel_service(config.port_forward_limits);
}

ServerRuntime::~ServerRuntime() {
    request_shutdown();
    join();
}

void ServerRuntime::start_accept_loop() {
    {
        BasicLockGuard lock(mutex_);
        if (accept_thread_.get() != nullptr || maintenance_thread_.get() != nullptr) {
            throw std::runtime_error("server runtime accept loop already started");
        }
        if (listener_.valid()) {
            throw std::runtime_error("server runtime listener already initialized");
        }

        listener_.reset(create_listener(state_.config));
    }

    accept_thread_.reset(new std::thread(&ServerRuntime::accept_loop, this));
    maintenance_thread_.reset(new std::thread(&ServerRuntime::maintenance_loop, this));
}

void ServerRuntime::request_shutdown() {
    {
        BasicLockGuard lock(mutex_);
        shutting_down_ = true;
    }
    shutdown_wakeup_.signal();

    connections_.begin_shutdown();
    if (state_.port_tunnel_service) {
        state_.port_tunnel_service->shutdown();
    }
}

void ServerRuntime::join() {
    std::unique_ptr<std::thread> accept_thread;
    std::unique_ptr<std::thread> maintenance_thread;
    {
        BasicLockGuard lock(mutex_);
        accept_thread.swap(accept_thread_);
        maintenance_thread.swap(maintenance_thread_);
    }

    join_daemon_thread(&accept_thread);
    join_daemon_thread(&maintenance_thread);

    connections_.begin_shutdown();
    connections_.wait_for_all();
    maintenance_once();
}

unsigned short ServerRuntime::bound_port() const {
    BasicLockGuard lock(mutex_);
    return socket_bound_port_or_zero(listener_.get());
}

AppState& ServerRuntime::state() {
    return state_;
}

ConnectionManager& ServerRuntime::connection_manager() {
    return connections_;
}

void ServerRuntime::maintenance_once() {
    connections_.reap_finished();

    bool shutting_down = false;
    {
        BasicLockGuard lock(mutex_);
        shutting_down = shutting_down_;
    }
    if (!shutting_down) {
        return;
    }

    connections_.wait_for_all();
}

void ServerRuntime::maintenance_loop() {
    for (;;) {
        maintenance_once();

        bool shutting_down = false;
        {
            BasicLockGuard lock(mutex_);
            shutting_down = shutting_down_;
        }
        if (shutting_down) {
            return;
        }

        platform::sleep_ms(250UL);
    }
}

void ServerRuntime::accept_loop() {
    SOCKET listener_socket = INVALID_SOCKET;
    SOCKET wakeup_fd = shutdown_wakeup_.read_fd();
    {
        BasicLockGuard lock(mutex_);
        listener_socket = listener_.get();
    }
    if (listener_socket == INVALID_SOCKET) {
        return;
    }
    set_socket_nonblocking(listener_socket, true);

    for (;;) {
        const int ready = wait_socket_readable_or_wakeup(listener_socket, wakeup_fd, 1000UL);
        if (ready < 0) {
            return;
        }
        if (ready == 0) {
            BasicLockGuard lock(mutex_);
            if (shutting_down_) {
                return;
            }
            continue;
        }

        for (;;) {
            UniqueSocket client(accept_client(listener_socket));
            if (!client.valid()) {
                const int error = last_socket_error();
                if (receive_timeout_error(error)) {
                    break;
                }
#ifndef _WIN32
                if (error == ECONNABORTED) {
                    break;
                }
#endif
                BasicLockGuard lock(mutex_);
                if (shutting_down_) {
                    return;
                }
                log_message(LOG_WARN, "server", "accept failed");
                break;
            }
            try {
                set_socket_nonblocking(client.get(), false);
            } catch (const std::exception& ex) {
                log_message(LOG_WARN, "server", std::string("accepted socket setup failed: ") + ex.what());
                continue;
            }

            if (!connections_.try_start(std::move(client), [this](SOCKET socket) {
                    UniqueSocket client(socket);
                    handle_client(this->state(), std::move(client));
                })) {
                log_message(LOG_WARN, "server", "dropping client connection during shutdown");
            }
        }
    }
}
