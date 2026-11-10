#include "runtime/reactor_server.h"

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <limits>
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
#include "tls/tls_connection_transport.h"

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <netinet/in.h>
#endif

namespace {

const int MAX_ACCEPTS_PER_EVENT = 64;
const unsigned long REVERSE_REGISTRATION_TIMEOUT_MS = 15000UL;
const char REVERSE_MAGIC[] = "REXREV1\n";

bool connect_in_progress_error(int error) {
#ifdef _WIN32
    return error == WSAEWOULDBLOCK || error == WSAEINPROGRESS;
#else
    return error == EINPROGRESS || error == EWOULDBLOCK || error == EAGAIN;
#endif
}

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

std::size_t tunnel_socket_capacity(const DaemonConfig& config) {
    const std::size_t max = std::numeric_limits<std::size_t>::max();
    std::size_t total = 64U;
    const unsigned long limits[] = {
        config.port_forward_limits.max_active_tcp_streams,
        config.port_forward_limits.max_retained_listeners,
        config.port_forward_limits.max_udp_binds,
        config.port_forward_limits.max_retained_sessions,
    };
    for (std::size_t i = 0; i < sizeof(limits) / sizeof(limits[0]); ++i) {
        const std::size_t value = static_cast<std::size_t>(limits[i]);
        total = value > max - total ? max : total + value;
    }
    // Tunnel control connections are not part of these resource limits, so
    // keep the loop at least as large as the POSIX backend's normal ceiling.
    const std::size_t minimum = 4096U;
    return total < minimum ? minimum : total;
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

struct ReverseConn : public std::enable_shared_from_this<ReverseConn> {
    enum State {
        Connecting,
        WritingRegistration,
        ReadingAck,
        Done,
    };

    ReverseConn(ReactorServer* server_value, std::uint64_t id_value)
        : server(server_value), id(id_value), socket(), stream(), state(Connecting),
          registration_offset(0U), timer(0U), timer_armed(false), finished(false) {}

    ~ReverseConn() {
        if (!finished) {
            finished = true;
            cancel_timer();
            if (stream) {
                server->loops_[0]->remove(stream->native_fd());
                stream->close();
            } else if (socket.valid()) {
                server->loops_[0]->remove(socket.get());
                socket.reset();
            }
        }
    }

    void start(const SocketAddress& address) {
        socket.reset(create_socket_cloexec(address.family, address.socktype, address.protocol));
        if (!socket.valid() || !socket_set_nonblocking(socket.get(), true)) {
            fail();
            return;
        }
        const int result =
            connect_socket(socket.get(), address.sockaddr_ptr(), address.address_len);
        if (result == 0) {
            on_connected();
            return;
        }
        if (!connect_in_progress_error(last_socket_error())) {
            fail();
            return;
        }
        std::shared_ptr<ReverseConn> self = shared_from_this();
        if (!server->loops_[0]->add(socket.get(), IO_WRITE, [self](int) { self->on_events(); })) {
            fail();
            return;
        }
        arm_timer();
    }

    void on_events() {
        if (finished) {
            return;
        }
        if (state == Connecting) {
            on_connected();
            return;
        }
        if (state == WritingRegistration) {
            on_write();
            return;
        }
        if (state == ReadingAck) {
            on_read();
        }
    }

    void on_connected() {
        int socket_error = 0;
        if (socket_error_option(socket.get(), &socket_error) != 0 || socket_error != 0) {
            fail();
            return;
        }
        server->loops_[0]->remove(socket.get());
        try {
            if (server->config_.reverse_transport == Transport::Tls) {
                stream = make_tls_client_stream(
                    std::move(socket),
                    server->tls_client_context_,
                    server->config_.reverse_tls_server_name
                );
            } else {
                stream = make_socket_stream(std::move(socket));
            }
        } catch (const std::exception&) {
            fail();
            return;
        }
        state = WritingRegistration;
        std::shared_ptr<ReverseConn> self = shared_from_this();
        if (!server->loops_[0]->add(stream->native_fd(), IO_WRITE, [self](int) {
                self->on_events();
            })) {
            fail();
            return;
        }
        arm_timer();
        on_write();
    }

    void on_write() {
        while (registration_offset < registration.size()) {
            const WriteResult result = stream->write(
                registration.data() + registration_offset,
                registration.size() - registration_offset
            );
            if (result.status == IoStatus::Ok) {
                registration_offset += result.bytes;
                continue;
            }
            if (result.status == IoStatus::WouldBlock) {
                update_interest(IO_WRITE);
                return;
            }
            fail();
            return;
        }
        state = ReadingAck;
        update_interest(IO_READ);
        on_read();
    }

    void on_read() {
        for (;;) {
            std::size_t wanted = 12U;
            if (ack.size() >= 12U) {
                const std::uint32_t length =
                    (static_cast<std::uint32_t>(static_cast<unsigned char>(ack[8])) << 24)
                    | (static_cast<std::uint32_t>(static_cast<unsigned char>(ack[9])) << 16)
                    | (static_cast<std::uint32_t>(static_cast<unsigned char>(ack[10])) << 8)
                    | static_cast<std::uint32_t>(static_cast<unsigned char>(ack[11]));
                if (length > 16384U) {
                    fail();
                    return;
                }
                wanted = 12U + length;
                if (ack.size() >= wanted) {
                    if (ack.find("\"accepted\":true") == std::string::npos) {
                        fail();
                        return;
                    }
                    finish();
                    return;
                }
            }

            char buffer[4096];
            const std::size_t ask = std::min<std::size_t>(sizeof(buffer), wanted - ack.size());
            const ReadResult result = stream->read(buffer, ask);
            if (result.status == IoStatus::Ok) {
                ack.append(buffer, result.bytes);
                continue;
            }
            if (result.status == IoStatus::WouldBlock) {
                update_interest(IO_READ);
                return;
            }
            fail();
            return;
        }
    }

    void update_interest(int fallback) {
        const int want = stream ? stream->wanted_events() : 0;
        const SOCKET fd = stream ? stream->native_fd() : socket.get();
        server->loops_[0]->set_interest(fd, want != 0 ? want : fallback);
    }

    void finish() {
        if (finished) {
            return;
        }
        finished = true;
        cancel_timer();
        server->loops_[0]->remove(stream->native_fd());
        std::shared_ptr<Stream> handed = stream;
        stream.reset();
        server->reverse_attempt_finished(true);
        server->serve_reverse_connection(handed);
    }

    void fail() {
        if (finished) {
            return;
        }
        finished = true;
        cancel_timer();
        if (stream) {
            server->loops_[0]->remove(stream->native_fd());
            stream->close();
        } else if (socket.valid()) {
            server->loops_[0]->remove(socket.get());
            socket.reset();
        }
        log_message(LOG_WARN, "server", "reverse connection attempt failed");
        server->reverse_attempt_finished(false);
    }

    void arm_timer() {
        cancel_timer();
        std::weak_ptr<ReverseConn> weak = shared_from_this();
        timer = server->loops_[0]->add_timer(
            platform::monotonic_ms() + REVERSE_REGISTRATION_TIMEOUT_MS,
            [weak]() {
                std::shared_ptr<ReverseConn> conn = weak.lock();
                if (conn) {
                    conn->timer_armed = false;
                    conn->fail();
                }
            }
        );
        timer_armed = true;
    }

    void cancel_timer() {
        if (!timer_armed) {
            return;
        }
        server->loops_[0]->cancel_timer(timer);
        timer_armed = false;
    }

    ReactorServer* server;
    std::uint64_t id;
    UniqueSocket socket;
    std::shared_ptr<Stream> stream;
    State state;
    std::string registration;
    std::size_t registration_offset;
    std::string ack;
    EventLoop::TimerId timer;
    bool timer_armed;
    bool finished;
};

ReactorServer::ReactorServer(const DaemonConfig& config)
    : config_(config), metadata_(), sandbox_(), loops_(), loop_threads_(), tunnel_loop_(),
      tunnel_thread_(),
      pool_(new BlockingPool(config.blocking_workers, config.blocking_workers * 8U + 8U)), exec_(),
      router_(), listener_(), connections_mutex_(), connections_(), shard_connections_(),
      next_shard_(0U), next_connection_id_(1U), bound_port_(0U), listener_paused_(false),
      shutting_down_(false), finalized_(false), reverse_live_(0UL), reverse_busy_(0UL),
      reverse_max_(0UL), reverse_connect_in_progress_(false), reverse_next_attempt_ms_(0U),
      next_reverse_id_(1U) {
    if (config_.transport == Transport::Tls && config_.connection_mode == ConnectionMode::Listen) {
        tls_server_context_ = make_tls_server_context(config_);
    }
    if (config_.connection_mode == ConnectionMode::Reverse
        && config_.reverse_transport == Transport::Tls) {
        tls_client_context_ = make_tls_client_context(config_);
    }

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
    tunnel_loop_.reset(new EventLoop(tunnel_socket_capacity(config_)));
    tunnel_.reset(new TunnelService(
        config_.port_forward_limits,
        config_.http_auth_bearer_token,
        tunnel_loop_.get(),
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
    if (!tunnel_loop_ || !tunnel_loop_->valid()) {
        if (error != nullptr) {
            *error = "tunnel event loop initialization failed";
        }
        return false;
    }

    std::size_t capacity = 0U;
    for (std::size_t i = 0; i < loops_.size(); ++i) {
        capacity += shard_capacity(i);
    }

    if (config_.connection_mode == ConnectionMode::Reverse) {
        reverse_max_ = std::min<unsigned long>(
            config_.reverse_max_connections,
            static_cast<unsigned long>(capacity)
        );
        if (reverse_max_ == 0UL) {
            if (error != nullptr) {
                *error = "reverse_max_connections exceeds the event loop shard capacity";
            }
            return false;
        }
        schedule_exec_tick();
        schedule_tunnel_tick();
        return true;
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
    schedule_tunnel_tick();
    return true;
}

void ReactorServer::run() {
    if (loops_.empty()) {
        return;
    }
    for (std::size_t i = 1; i < loops_.size(); ++i) {
        loop_threads_.push_back(std::thread([this, i]() { loops_[i]->run(); }));
    }
    if (tunnel_loop_) {
        tunnel_thread_ = std::thread([this]() { tunnel_loop_->run(); });
    }
    loops_[0]->run();
    stop_loops();
    for (std::size_t i = 0; i < loop_threads_.size(); ++i) {
        if (loop_threads_[i].joinable()) {
            loop_threads_[i].join();
        }
    }
    loop_threads_.clear();
    if (tunnel_thread_.joinable()) {
        tunnel_thread_.join();
    }
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
    if (tunnel_loop_) {
        tunnel_loop_->post([this]() { tunnel_loop_->stop(); });
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
    reverse_tick();
    schedule_exec_tick();
}

void ReactorServer::schedule_tunnel_tick() {
    if (shutting_down_.load() || !tunnel_loop_) {
        return;
    }
    tunnel_loop_->add_timer(platform::monotonic_ms() + 20U, [this]() { tunnel_tick(); });
}

void ReactorServer::tunnel_tick() {
    if (tunnel_) {
        tunnel_->tick();
    }
    schedule_tunnel_tick();
}

void ReactorServer::reverse_tick() {
    if (config_.connection_mode != ConnectionMode::Reverse || shutting_down_.load()) {
        return;
    }
    const std::uint64_t now = platform::monotonic_ms();
    if (reverse_connect_in_progress_ || now < reverse_next_attempt_ms_) {
        return;
    }
    const unsigned long live = reverse_live_.load();
    const unsigned long busy = reverse_busy_.load();
    if (live >= reverse_max_) {
        return;
    }
    if (live - std::min(live, busy) >= config_.reverse_min_idle_connections) {
        return;
    }
    start_reverse_connect();
}

void ReactorServer::start_reverse_connect() {
    reverse_connect_in_progress_ = true;
    const std::string host = config_.reverse_broker_host;
    const std::string service = std::to_string(config_.reverse_broker_port);
    const bool posted = pool_->post([this, host, service]() {
        SocketAddressQuery query;
        query.family = AF_UNSPEC;
        query.socktype = SOCK_STREAM;
        query.protocol = IPPROTO_TCP;
        query.passive = false;
        std::vector<SocketAddress> addresses;
        std::string resolve_error;
        const bool resolved = resolve_socket_addresses(
            host.c_str(),
            service.c_str(),
            query,
            &addresses,
            &resolve_error
        );
        loops_[0]->post([this, addresses, resolved, resolve_error]() {
            if (!resolved || addresses.empty()) {
                log_message(LOG_WARN, "server", "reverse broker resolve failed: " + resolve_error);
                reverse_attempt_finished(false);
                return;
            }
            begin_reverse_connect(addresses);
        });
    });
    if (!posted) {
        reverse_attempt_finished(false);
    }
}

void ReactorServer::begin_reverse_connect(const std::vector<SocketAddress>& addresses) {
    if (shutting_down_.load() || addresses.empty()) {
        reverse_attempt_finished(false);
        return;
    }
    std::shared_ptr<ReverseConn> conn(new ReverseConn(this, next_reverse_id_++));
    const std::string payload =
        "{\"protocol_version\":1,\"target\":\"" + json_escape(config_.target)
        + "\",\"daemon_instance_id\":\"" + json_escape(metadata_.daemon_instance_id)
        + "\",\"bearer_token\":\"" + json_escape(config_.reverse_bearer_token) + "\"}";
    const std::uint32_t payload_size = static_cast<std::uint32_t>(payload.size());
    conn->registration.assign(REVERSE_MAGIC, 8U);
    conn->registration.push_back(static_cast<char>((payload_size >> 24U) & 0xffU));
    conn->registration.push_back(static_cast<char>((payload_size >> 16U) & 0xffU));
    conn->registration.push_back(static_cast<char>((payload_size >> 8U) & 0xffU));
    conn->registration.push_back(static_cast<char>(payload_size & 0xffU));
    conn->registration += payload;
    conn->start(addresses[0]);
}

void ReactorServer::serve_reverse_connection(const std::shared_ptr<Stream>& stream) {
    std::size_t shard = 0U;
    if (!assign_shard(&shard)) {
        stream->close();
        return;
    }
    reverse_live_.fetch_add(1UL);

    std::shared_ptr<std::atomic<bool>> busy(new std::atomic<bool>(false));
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
    options.on_first_request = [this, busy]() {
        if (!busy->exchange(true)) {
            reverse_busy_.fetch_add(1UL);
        }
    };
    const std::uint64_t id = next_connection_id_++;
    options.on_closed = [this, busy](std::uint64_t closed_id) {
        on_conn_closed(closed_id);
        if (busy->load()) {
            reverse_busy_.fetch_sub(1UL);
        }
        reverse_live_.fetch_sub(1UL);
    };

    std::shared_ptr<HttpConn> connection(new HttpConn(id, shard, stream, options));
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

void ReactorServer::reverse_attempt_finished(bool success) {
    reverse_connect_in_progress_ = false;
    if (!success) {
        reverse_next_attempt_ms_ =
            platform::monotonic_ms() + static_cast<std::uint64_t>(config_.reverse_reconnect_ms);
    } else {
        reverse_next_attempt_ms_ = 0U;
    }
}

void ReactorServer::stop_loops() {
    for (std::size_t i = 0; i < loops_.size(); ++i) {
        loops_[i]->stop();
    }
    if (tunnel_loop_) {
        tunnel_loop_->stop();
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

        std::shared_ptr<Stream> stream;
        try {
            if (config_.transport == Transport::Tls) {
                stream = make_tls_server_stream(UniqueSocket(accepted), tls_server_context_);
            } else {
                stream = make_socket_stream(UniqueSocket(accepted));
            }
        } catch (const std::exception& ex) {
            close_socket(accepted);
            log_message(
                LOG_WARN,
                "server",
                std::string("client stream setup failed: ") + ex.what()
            );
            continue;
        }

        std::shared_ptr<HttpConn> connection(new HttpConn(id, shard, stream, options));
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
