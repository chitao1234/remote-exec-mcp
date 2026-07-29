#include "runtime/server_runtime.h"

#include <algorithm>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <thread>
#include <vector>

#include "../port_forward/port_tunnel_service.h"
#include "capabilities/daemon_capabilities.h"
#include "core/logging.h"
#include "http/http_connection.h"
#include "http/server_transport.h"
#include "platform/platform.h"
#include "policy/path_policy.h"
#include "port_forward/port_forward_socket_ops.h"
#include "port_forward/port_tunnel.h"
#include "runtime/app_context.h"
#include "runtime/daemon_thread.h"
#include "tls/tls_connection_transport.h"

namespace {

const char REVERSE_MAGIC[] = "REXREV1\n";

std::string json_escape(const std::string& value) {
    std::string result;
    for (std::size_t i = 0; i < value.size(); ++i) {
        const char ch = value[i];
        if (ch == '\\' || ch == '"') {
            result.push_back('\\');
        }
        result.push_back(ch);
    }
    return result;
}

bool recv_exact(SOCKET socket, char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int received = recv_bounded(socket, data + offset, size - offset, 0);
        if (received <= 0) {
            return false;
        }
        offset += static_cast<std::size_t>(received);
    }
    return true;
}

UniqueSocket connect_reverse_socket(const DaemonConfig& config, const AppMetadata& metadata) {
    SocketAddressQuery query;
    query.family = AF_UNSPEC;
    query.socktype = SOCK_STREAM;
    std::vector<SocketAddress> addresses;
    std::string error;
    const std::string service = std::to_string(config.reverse_broker_port);
    if (!resolve_socket_addresses(
            config.reverse_broker_host.c_str(),
            service.c_str(),
            query,
            &addresses,
            &error
        )) {
        throw std::runtime_error(error);
    }
    UniqueSocket socket;
    for (std::size_t i = 0; i < addresses.size(); ++i) {
        socket.reset(
            create_socket_cloexec(addresses[i].family, addresses[i].socktype, addresses[i].protocol)
        );
        if (socket.valid()
            && connect_socket(socket.get(), addresses[i].sockaddr_ptr(), addresses[i].address_len)
                   == 0) {
            break;
        }
        socket.reset();
    }
    if (!socket.valid()) {
        throw std::runtime_error(socket_error_message("connect reverse broker"));
    }

    const std::string payload =
        "{\"protocol_version\":1,\"target\":\"" + json_escape(config.target)
        + "\",\"daemon_instance_id\":\"" + json_escape(metadata.daemon_instance_id)
        + "\",\"bearer_token\":\"" + json_escape(config.reverse_bearer_token) + "\"}";
    char length[4];
    const std::uint32_t payload_size = static_cast<std::uint32_t>(payload.size());
    length[0] = static_cast<char>((payload_size >> 24U) & 0xffU);
    length[1] = static_cast<char>((payload_size >> 16U) & 0xffU);
    length[2] = static_cast<char>((payload_size >> 8U) & 0xffU);
    length[3] = static_cast<char>(payload_size & 0xffU);
    send_all_bytes(socket.get(), REVERSE_MAGIC, 8U);
    send_all_bytes(socket.get(), length, sizeof(length));
    send_all(socket.get(), payload);

    char ack_magic[8];
    char ack_length[4];
    if (!recv_exact(socket.get(), ack_magic, sizeof(ack_magic))
        || std::memcmp(ack_magic, REVERSE_MAGIC, sizeof(ack_magic)) != 0
        || !recv_exact(socket.get(), ack_length, sizeof(ack_length))) {
        throw std::runtime_error("invalid reverse registration acknowledgement");
    }
    const std::uint32_t ack_size = (static_cast<unsigned char>(ack_length[0]) << 24U)
                                   | (static_cast<unsigned char>(ack_length[1]) << 16U)
                                   | (static_cast<unsigned char>(ack_length[2]) << 8U)
                                   | static_cast<unsigned char>(ack_length[3]);
    if (ack_size > 16384U) {
        throw std::runtime_error("reverse registration acknowledgement is too large");
    }
    std::string ack(ack_size, '\0');
    if (!recv_exact(socket.get(), &ack[0], ack.size())
        || ack.find("\"accepted\":true") == std::string::npos) {
        throw std::runtime_error("broker rejected reverse registration");
    }
    return socket;
}

std::string daemon_instance_id() {
    std::ostringstream out;
    out << platform::monotonic_ms();
    return out.str();
}

} // namespace

ServerRuntime::ServerRuntime(const DaemonConfig& config)
    : config_(config), connections_(config.max_open_sessions), shutting_down_(false),
      accept_thread_(), maintenance_thread_(), reverse_live_(0UL), reverse_busy_(0UL) {
    metadata_.daemon_instance_id = daemon_instance_id();
    metadata_.hostname = platform::hostname();
    metadata_.default_shell = platform::resolve_default_shell(config.default_shell);
    metadata_.capabilities = detect_daemon_capabilities();
    sandbox_.enabled = config.sandbox_configured;
    if (sandbox_.enabled) {
        sandbox_.compiled = compile_filesystem_sandbox(config.sandbox);
    }
    services_.port_tunnel = create_port_tunnel_service(config.port_forward_limits);
    if (config_.connection_mode == "listen" && config_.transport == "tls") {
        tls_server_context_ = make_tls_server_context(config_);
    }
    if (config_.connection_mode == "reverse" && config_.reverse_transport == "tls") {
        tls_client_context_ = make_tls_client_context(config_);
    }
}

void ServerRuntime::start_reverse_loop() {
    BasicLockGuard lock(mutex_);
    if (accept_thread_.get() != nullptr || maintenance_thread_.get() != nullptr) {
        throw std::runtime_error("server runtime already started");
    }
    accept_thread_.reset(new std::thread(&ServerRuntime::reverse_loop, this));
    maintenance_thread_.reset(new std::thread(&ServerRuntime::maintenance_loop, this));
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

        listener_.reset(create_listener(config_));
    }

    accept_thread_.reset(new std::thread(&ServerRuntime::accept_loop, this));
    maintenance_thread_.reset(new std::thread(&ServerRuntime::maintenance_loop, this));
}

void ServerRuntime::request_shutdown() {
    {
        BasicLockGuard lock(mutex_);
        shutting_down_ = true;
    }
    shutdown_.requested.store(true);
    shutdown_wakeup_.signal();

    connections_.begin_shutdown();
    if (services_.port_tunnel) {
        services_.port_tunnel->shutdown();
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

const DaemonConfig& ServerRuntime::config() const {
    return config_;
}

const AppMetadata& ServerRuntime::metadata() const {
    return metadata_;
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
                log_message(
                    LOG_WARN,
                    "server",
                    std::string("accepted socket setup failed: ") + ex.what()
                );
                continue;
            }

            if (!connections_.try_start(std::move(client), [this](SOCKET socket) {
                    try {
                        std::shared_ptr<ConnectionTransport> client =
                            this->config_.transport == "tls"
                                ? make_tls_server_connection_transport(
                                      UniqueSocket(socket),
                                      this->tls_server_context_,
                                      this->config_.tls_handshake_timeout_ms
                                  )
                                : make_plain_connection_transport(UniqueSocket(socket));
                        handle_client(
                            make_http_connection_context(
                                this->config_,
                                this->metadata_,
                                this->sandbox_,
                                this->services_,
                                this->shutdown_
                            ),
                            client
                        );
                    } catch (const std::exception& ex) {
                        log_message(
                            LOG_WARN,
                            "server",
                            std::string("client connection failed: ") + ex.what()
                        );
                    }
                })) {
                log_message(LOG_WARN, "server", "dropping client connection during shutdown");
            }
        }
    }
}

void ServerRuntime::reverse_loop() {
    for (;;) {
        {
            BasicLockGuard lock(mutex_);
            if (shutting_down_) {
                return;
            }
        }
        const unsigned long live = reverse_live_.load();
        const unsigned long busy = reverse_busy_.load();
        if (live >= config_.reverse_max_connections
            || live - std::min(live, busy) >= config_.reverse_min_idle_connections) {
            platform::sleep_ms(100UL);
            continue;
        }
        try {
            UniqueSocket client = connect_reverse_socket(config_, metadata_);
            reverse_live_.fetch_add(1UL);
            if (!connections_.try_start(std::move(client), [this](SOCKET socket) {
                    std::shared_ptr<std::atomic<bool>> became_busy(new std::atomic<bool>(false));
                    std::shared_ptr<ConnectionTransport> client =
                        make_plain_connection_transport(UniqueSocket(socket));
                    handle_client(
                        make_http_connection_context(
                            this->config_,
                            this->metadata_,
                            this->sandbox_,
                            this->services_,
                            this->shutdown_
                        ),
                        client,
                        [this, became_busy]() {
                            if (!became_busy->exchange(true)) {
                                this->reverse_busy_.fetch_add(1UL);
                            }
                        }
                    );
                    if (became_busy->load()) {
                        this->reverse_busy_.fetch_sub(1UL);
                    }
                    this->reverse_live_.fetch_sub(1UL);
                })) {
                reverse_live_.fetch_sub(1UL);
                return;
            }
        } catch (const std::exception& ex) {
            log_message(LOG_WARN, "server", std::string("reverse connection failed: ") + ex.what());
            platform::sleep_ms(config_.reverse_reconnect_ms);
        }
    }
}
