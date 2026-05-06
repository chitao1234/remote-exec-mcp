#include "server_runtime.h"

#include <cstring>
#include <sstream>
#include <stdexcept>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#else
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <thread>
#endif

#include "logging.h"
#include "path_policy.h"
#include "platform.h"

namespace {

std::string daemon_instance_id() {
    std::ostringstream out;
    out << platform::monotonic_ms();
    return out.str();
}

unsigned short socket_bound_port_or_zero(SOCKET socket) {
    if (socket == INVALID_SOCKET) {
        return 0;
    }

    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    socklen_t address_len = sizeof(address);
    if (getsockname(socket, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        return 0;
    }

    if (address.ss_family == AF_INET) {
        const sockaddr_in* ipv4 = reinterpret_cast<const sockaddr_in*>(&address);
        return ntohs(ipv4->sin_port);
    }
    if (address.ss_family == AF_INET6) {
        const sockaddr_in6* ipv6 = reinterpret_cast<const sockaddr_in6*>(&address);
        return ntohs(ipv6->sin6_port);
    }

    return 0;
}

void connection_worker_main(SOCKET socket, void* context) {
    ServerRuntime* runtime = static_cast<ServerRuntime*>(context);
    UniqueSocket client(socket);
    handle_client(runtime->state(), std::move(client));
}

}  // namespace

ServerRuntime::ServerRuntime(const DaemonConfig& config)
    : connections_(config.max_open_sessions),
      shutting_down_(false)
#ifdef _WIN32
      ,
      accept_thread_(NULL)
#else
      ,
      accept_thread_(NULL)
#endif
{
    state_.config = config;
    state_.daemon_instance_id = daemon_instance_id();
    state_.hostname = platform::hostname();
    state_.default_shell = platform::resolve_default_shell(config.default_shell);
    state_.sandbox_enabled = config.sandbox_configured;
    if (state_.sandbox_enabled) {
        state_.sandbox = compile_filesystem_sandbox(host_path_policy(), config.sandbox);
    }
}

ServerRuntime::~ServerRuntime() {
    request_shutdown();
    join();
}

void ServerRuntime::start_accept_loop() {
    BasicLockGuard lock(mutex_);
    if (accept_thread_ != NULL) {
        throw std::runtime_error("server runtime accept loop already started");
    }
    if (listener_.valid()) {
        throw std::runtime_error("server runtime listener already initialized");
    }

    listener_.reset(create_listener(state_.config));

#ifdef _WIN32
    accept_thread_ = CreateThread(NULL, 0, &ServerRuntime::accept_thread_entry, this, 0, NULL);
    if (accept_thread_ == NULL) {
        listener_.reset();
        throw std::runtime_error("CreateThread failed");
    }
#else
    accept_thread_ = new std::thread(&ServerRuntime::accept_loop, this);
#endif
}

void ServerRuntime::request_shutdown() {
    SOCKET listener_socket = INVALID_SOCKET;
    {
        BasicLockGuard lock(mutex_);
        shutting_down_ = true;
        listener_socket = listener_.release();
    }

    if (listener_socket != INVALID_SOCKET) {
        shutdown_socket(listener_socket);
        close_socket(listener_socket);
    }

    connections_.begin_shutdown();
}

void ServerRuntime::join() {
#ifdef _WIN32
    HANDLE accept_thread = NULL;
#else
    std::thread* accept_thread = NULL;
#endif
    {
        BasicLockGuard lock(mutex_);
        accept_thread = accept_thread_;
        accept_thread_ = NULL;
    }

#ifdef _WIN32
    if (accept_thread != NULL) {
        WaitForSingleObject(accept_thread, INFINITE);
        CloseHandle(accept_thread);
    }
#else
    if (accept_thread != NULL) {
        accept_thread->join();
        delete accept_thread;
    }
#endif

    connections_.begin_shutdown();
    while (connections_.active_count() != 0UL) {
        maintenance_once();
        platform::sleep_ms(10UL);
    }
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
}

void ServerRuntime::accept_loop() {
    SOCKET listener_socket = INVALID_SOCKET;
    {
        BasicLockGuard lock(mutex_);
        listener_socket = listener_.get();
    }

    while (listener_socket != INVALID_SOCKET) {
        UniqueSocket client(accept_client(listener_socket));
        if (!client.valid()) {
            bool shutting_down = false;
            {
                BasicLockGuard lock(mutex_);
                shutting_down = shutting_down_;
            }
            if (shutting_down) {
                return;
            }
            log_message(LOG_WARN, "server", "accept failed");
            continue;
        }

        if (!connections_.try_start(std::move(client), &connection_worker_main, this)) {
            log_message(LOG_WARN, "server", "dropping client connection during shutdown");
        }
    }
}

#ifdef _WIN32
DWORD WINAPI ServerRuntime::accept_thread_entry(LPVOID raw_context) {
    ServerRuntime* runtime = static_cast<ServerRuntime*>(raw_context);
    runtime->accept_loop();
    return 0;
}
#endif
