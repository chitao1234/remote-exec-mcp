#include "runtime/reactor_server.h"

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <sstream>
#include <stdexcept>
#include <utility>
#include <vector>

#include "capabilities/daemon_capabilities.h"
#include "core/blocking_pool.h"
#include "core/event_loop.h"
#include "core/logging.h"
#include "exec/exec_service.h"
#include "exec/process_session.h"
#include "http/reactor_http_conn.h"
#include "platform/platform.h"
#include "policy/filesystem_sandbox.h"
#include "port_forward/tunnel_service.h"
#include "rpc/reactor_router.h"
#include "runtime/route_context.h"

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <netinet/in.h>
#endif

namespace {

const int MAX_ACCEPTS_PER_EVENT = 64;

std::string make_instance_id() {
    std::ostringstream out;
    out << platform::monotonic_ms();
    return out.str();
}

DaemonCapabilities reactor_capabilities(const DaemonConfig& config) {
    DaemonCapabilities capabilities = detect_daemon_capabilities(config);
    capabilities.supports_exec = config.allow_exec;
    capabilities.supports_pty = process_session_supports_pty();
    capabilities.supports_port_forward = true;
    capabilities.port_forward_protocol_version = server_contract::PORT_TUNNEL_PROTOCOL_VERSION;
    capabilities.transfer_stream_protocol_version =
        server_contract::TRANSFER_STREAM_PROTOCOL_VERSION;
    return capabilities;
}

SOCKET create_listener_socket(const DaemonConfig& config) {
    char port_buffer[16];
    std::snprintf(port_buffer, sizeof(port_buffer), "%d", config.listen_port);

    SocketAddressQuery query;
    query.family = AF_INET;
    query.socktype = SOCK_STREAM;
    query.protocol = IPPROTO_TCP;
    query.passive = true;

    std::vector<SocketAddress> addresses;
    std::string resolve_error;
    if (!resolve_socket_addresses(
            config.listen_host.c_str(),
            port_buffer,
            query,
            &addresses,
            &resolve_error
        )) {
        throw std::runtime_error("resolve listen address failed: " + resolve_error);
    }

    SOCKET listener = try_create_socket_for_addresses(
        addresses,
        [](SOCKET created, const SocketAddress& current) {
            (void)set_socket_reuseaddr(created);
            return bind_socket(created, current.sockaddr_ptr(), current.address_len) == 0;
        }
    );
    if (listener == INVALID_SOCKET) {
        throw std::runtime_error(socket_error_message("bind"));
    }
    if (listen_socket(listener, SOMAXCONN) != 0) {
        const std::string message = socket_error_message("listen");
        close_socket(listener);
        throw std::runtime_error(message);
    }
    if (!socket_set_nonblocking(listener, true)) {
        close_socket(listener);
        throw std::runtime_error("failed to set listener non-blocking");
    }
    return listener;
}

std::size_t required_shards(const DaemonConfig& config) {
    const std::size_t per_shard = EventLoop::max_watched_sockets();
    const std::size_t configured = config.io_threads == 0UL ? 1U : config.io_threads;
    if (per_shard <= 1U) {
        return configured;
    }
    const std::size_t first_capacity = per_shard - 1U;
    if (config.max_connections <= first_capacity) {
        return configured;
    }
    const std::size_t remaining = config.max_connections - first_capacity;
    const std::size_t extra = (remaining + per_shard - 1U) / per_shard;
    const std::size_t required = 1U + extra;
    return required > configured ? required : configured;
}

} // namespace

ReactorServer::ReactorServer(const DaemonConfig& config)
    : config_(config), metadata_(), sandbox_(), loops_(), loop_threads_(),
      pool_(new BlockingPool(config.blocking_workers, config.blocking_workers * 8U + 8U)), exec_(),
      router_(), listener_(), connections_mutex_(), connections_(), shard_connections_(),
      next_shard_(0U), next_connection_id_(1U), bound_port_(0U), listener_paused_(false),
      shutting_down_(false), finalized_(false) {
    const std::size_t shards = required_shards(config_);
    loops_.reserve(shards);
    shard_connections_.reserve(shards);
    for (std::size_t i = 0; i < shards; ++i) {
        loops_.push_back(std::unique_ptr<EventLoop>(new EventLoop()));
        shard_connections_.push_back(0U);
    }

    metadata_.daemon_instance_id = make_instance_id();
    metadata_.hostname = platform::hostname();
    metadata_.default_shell =
        platform::resolve_default_shell(config_.default_shell, config_.windows_posix_root);
    metadata_.capabilities = reactor_capabilities(config_);
    sandbox_.enabled = config_.sandbox_configured;
    if (sandbox_.enabled) {
        sandbox_.compiled = compile_filesystem_sandbox(config_.sandbox);
    }

    const PathResolutionContext paths(
        config_.default_workdir,
        config_.windows_posix_root,
        sandbox_.active()
    );
    const ExecRequestContext exec_request(
        paths,
        metadata_.capabilities,
        metadata_.default_shell,
        config_.allow_login_shell
    );
    exec_.reset(new ExecService(
        exec_request,
        config_.target,
        metadata_.daemon_instance_id,
        config_.yield_time,
        config_.stdin_write_timeout_ms,
        config_.max_open_sessions,
        config_.allow_exec,
        loops_[0].get(),
        pool_.get()
    ));
    tunnel_.reset(new TunnelService(
        config_.port_forward_limits,
        config_.http_auth_bearer_token,
        loops_[0].get(),
        pool_.get()
    ));
    router_.reset(new ReactorRouter(config_, metadata_, sandbox_));
}

ReactorServer::~ReactorServer() {
    finalize();
}

std::size_t ReactorServer::shard_count() const {
    return loops_.size();
}

std::size_t ReactorServer::shard_capacity(std::size_t shard) const {
    const std::size_t per_shard = EventLoop::max_watched_sockets();
    if (per_shard <= 1U) {
        return 0U;
    }
    return shard == 0U ? per_shard - 1U : per_shard;
}

bool ReactorServer::assign_shard(std::size_t* shard) {
    BasicLockGuard lock(connections_mutex_);
    const std::size_t count = loops_.size();
    if (count == 0U) {
        return false;
    }
    for (std::size_t i = 0; i < count; ++i) {
        const std::size_t candidate = (next_shard_ + i) % count;
        if (shard_connections_[candidate] < shard_capacity(candidate)) {
            *shard = candidate;
            next_shard_ = (candidate + 1U) % count;
            return true;
        }
    }
    listener_paused_ = true;
    return false;
}

void ReactorServer::resume_listener() {
    if (!listener_.valid()) {
        return;
    }
    loops_[0]->set_interest(listener_.get(), IO_READ);
}

bool ReactorServer::start(std::string* error) {
    if (config_.connection_mode == ConnectionMode::Reverse) {
        if (error != nullptr) {
            *error = "reverse connection mode is not available on the reactor server yet";
        }
        return false;
    }
    if (config_.transport == Transport::Tls) {
        if (error != nullptr) {
            *error = "TLS transport is not available on the reactor server yet";
        }
        return false;
    }
    if (loops_.empty()) {
        if (error != nullptr) {
            *error = "no event loop shards configured";
        }
        return false;
    }
    for (std::size_t i = 0; i < loops_.size(); ++i) {
        if (!loops_[i]->valid()) {
            if (error != nullptr) {
                *error = "event loop shard initialization failed";
            }
            return false;
        }
    }

    std::size_t capacity = 0U;
    for (std::size_t i = 0; i < loops_.size(); ++i) {
        capacity += shard_capacity(i);
    }
    if (config_.max_connections > capacity) {
        if (error != nullptr) {
            *error = "max_connections exceeds the event loop shard capacity";
        }
        return false;
    }

    try {
        listener_.reset(create_listener_socket(config_));
    } catch (const std::exception& ex) {
        if (error != nullptr) {
            *error = ex.what();
        }
        return false;
    }

    if (!loops_[0]->add(listener_.get(), IO_READ, [this](int) { accept_ready(); })) {
        if (error != nullptr) {
            *error = "failed to register listener with the event loop";
        }
        listener_.reset();
        return false;
    }

    bound_port_ = socket_bound_port_or_zero(listener_.get());
    schedule_exec_tick();
    return true;
}

void ReactorServer::run() {
    if (loops_.empty()) {
        return;
    }
    for (std::size_t i = 1; i < loops_.size(); ++i) {
        loop_threads_.push_back(std::thread([this, i]() { loops_[i]->run(); }));
    }
    loops_[0]->run();
    stop_loops();
    for (std::size_t i = 0; i < loop_threads_.size(); ++i) {
        if (loop_threads_[i].joinable()) {
            loop_threads_[i].join();
        }
    }
    loop_threads_.clear();
    finalize();
}

void ReactorServer::request_stop() {
    shutting_down_.store(true);
    if (loops_.empty()) {
        return;
    }
    loops_[0]->post([this]() {
        if (listener_.valid()) {
            loops_[0]->remove(listener_.get());
            listener_.reset();
        }
    });
    for (std::size_t i = 0; i < loops_.size(); ++i) {
        loops_[i]->post([this, i]() { loops_[i]->stop(); });
    }
}

void ReactorServer::schedule_exec_tick() {
    if (shutting_down_.load() || loops_.empty()) {
        return;
    }
    loops_[0]->add_timer(platform::monotonic_ms() + 20U, [this]() { exec_tick(); });
}

void ReactorServer::exec_tick() {
    if (exec_) {
        exec_->tick();
    }
    if (tunnel_) {
        tunnel_->tick();
    }
    schedule_exec_tick();
}

void ReactorServer::stop_loops() {
    for (std::size_t i = 0; i < loops_.size(); ++i) {
        loops_[i]->stop();
    }
}

unsigned short ReactorServer::bound_port() const {
    return bound_port_;
}

const DaemonConfig& ReactorServer::config() const {
    return config_;
}

const AppMetadata& ReactorServer::metadata() const {
    return metadata_;
}

void ReactorServer::accept_ready() {
    if (!listener_.valid()) {
        return;
    }

    for (int i = 0; i < MAX_ACCEPTS_PER_EVENT; ++i) {
        std::size_t shard = 0U;
        if (!assign_shard(&shard)) {
            loops_[0]->set_interest(listener_.get(), 0);
            return;
        }

        SOCKET accepted = accept_socket_cloexec(listener_.get(), nullptr, nullptr);
        if (accepted == INVALID_SOCKET) {
            const int error = last_socket_error();
            if (would_block_error(error)) {
                return;
            }
#ifdef _WIN32
            if (error == WSAECONNABORTED) {
                continue;
            }
#else
            if (error == ECONNABORTED) {
                continue;
            }
#endif
            log_message(LOG_WARN, "server", socket_error_message("accept"));
            return;
        }

        if (!socket_set_nonblocking(accepted, true)) {
            close_socket(accepted);
            continue;
        }

        HttpConn::Options options;
        options.loop = loops_[shard].get();
        options.pool = pool_.get();
        options.router = router_.get();
        options.exec = exec_.get();
        options.tunnel = tunnel_.get();
        options.max_header_bytes = config_.max_request_header_bytes;
        options.max_body_bytes = config_.max_request_body_bytes;
        options.idle_timeout_ms = config_.http_connection_idle_timeout_ms;
        options.shutting_down = &shutting_down_;
        const std::uint64_t id = next_connection_id_++;
        options.on_closed = [this](std::uint64_t closed_id) { on_conn_closed(closed_id); };

        std::shared_ptr<HttpConn> connection(
            new HttpConn(id, shard, UniqueSocket(accepted), options)
        );
        {
            BasicLockGuard lock(connections_mutex_);
            connections_[id] = connection;
            ++shard_connections_[shard];
        }

        if (shard == 0U) {
            connection->start();
        } else {
            loops_[shard]->post([connection]() { connection->start(); });
        }
    }
}

void ReactorServer::on_conn_closed(std::uint64_t id) {
    bool resume = false;
    {
        BasicLockGuard lock(connections_mutex_);
        std::map<std::uint64_t, std::shared_ptr<HttpConn>>::iterator it = connections_.find(id);
        if (it == connections_.end()) {
            return;
        }
        const std::size_t shard = it->second->shard_index();
        connections_.erase(it);
        if (shard < shard_connections_.size() && shard_connections_[shard] > 0U) {
            --shard_connections_[shard];
        }
        if (listener_paused_) {
            listener_paused_ = false;
            resume = true;
        }
    }
    if (resume && !loops_.empty()) {
        loops_[0]->post([this]() { resume_listener(); });
    }
}

void ReactorServer::finalize() {
    if (finalized_) {
        return;
    }
    finalized_ = true;
    if (exec_) {
        exec_->cancel_all();
    }
    if (tunnel_) {
        tunnel_->shutdown();
    }
    if (pool_) {
        pool_->stop();
    }

    std::vector<std::shared_ptr<HttpConn>> connections;
    {
        BasicLockGuard lock(connections_mutex_);
        connections.reserve(connections_.size());
        for (std::map<std::uint64_t, std::shared_ptr<HttpConn>>::const_iterator it =
                 connections_.begin();
             it != connections_.end();
             ++it) {
            connections.push_back(it->second);
        }
        connections_.clear();
        for (std::size_t i = 0; i < shard_connections_.size(); ++i) {
            shard_connections_[i] = 0U;
        }
    }
    for (std::size_t i = 0; i < connections.size(); ++i) {
        connections[i]->close();
    }
    if (listener_.valid()) {
        listener_.reset();
    }
}
