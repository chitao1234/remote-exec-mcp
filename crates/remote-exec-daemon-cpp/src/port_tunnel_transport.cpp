#include "port_tunnel_internal.h"

#include <limits>

#ifdef REMOTE_EXEC_CPP_TESTING
static std::atomic<unsigned long> g_forced_tcp_read_thread_failures(0UL);

void set_forced_tcp_read_thread_failures(unsigned long count) {
    g_forced_tcp_read_thread_failures.store(count);
}

static bool consume_forced_tcp_read_thread_failure() {
    unsigned long current = g_forced_tcp_read_thread_failures.load();
    while (current > 0UL) {
        if (g_forced_tcp_read_thread_failures.compare_exchange_weak(current, current - 1UL)) {
            return true;
        }
    }
    return false;
}
#endif

TcpReadStartGate::TcpReadStartGate() : released_(false) {}

void TcpReadStartGate::release() {
    BasicLockGuard lock(mutex_);
    released_ = true;
    cond_.broadcast();
}

void TcpReadStartGate::wait() {
    BasicLockGuard lock(mutex_);
    while (!released_) {
        cond_.wait(mutex_);
    }
}

#ifdef _WIN32
struct TcpReadContext {
    std::shared_ptr<PortTunnelService> service;
    std::shared_ptr<PortTunnelConnection> tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelTcpStream> stream;
    std::shared_ptr<TcpReadStartGate> start_gate;
};

unsigned __stdcall tcp_read_thread_entry(void* raw_context) {
    std::unique_ptr<TcpReadContext> context(static_cast<TcpReadContext*>(raw_context));
    PortTunnelWorkerLease lease(context->service);
    if (context->start_gate.get() != NULL) {
        context->start_gate->wait();
    }
    context->tunnel->tcp_read_loop(context->stream_id, context->stream);
    return 0;
}

struct TcpWriteContext {
    std::shared_ptr<PortTunnelService> service;
    std::shared_ptr<PortTunnelConnection> tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelTcpStream> stream;
};

unsigned __stdcall tcp_write_thread_entry(void* raw_context) {
    std::unique_ptr<TcpWriteContext> context(static_cast<TcpWriteContext*>(raw_context));
    PortTunnelWorkerLease lease(context->service);
    context->tunnel->tcp_write_loop(context->stream_id, context->stream);
    return 0;
}
#endif

bool spawn_tcp_read_thread(
    const std::shared_ptr<PortTunnelService>& service,
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream,
    bool worker_acquired,
    const std::shared_ptr<TcpReadStartGate>& start_gate
) {
    if (!worker_acquired && !service->try_acquire_worker()) {
        return false;
    }
#ifdef REMOTE_EXEC_CPP_TESTING
    if (consume_forced_tcp_read_thread_failure()) {
        service->release_worker();
        return false;
    }
#endif
#ifdef _WIN32
    std::unique_ptr<TcpReadContext> context(new TcpReadContext());
    context->service = service;
    context->tunnel = tunnel;
    context->stream_id = stream_id;
    context->stream = stream;
    context->start_gate = start_gate;
    HANDLE handle = begin_win32_thread(tcp_read_thread_entry, context.get());
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
        return true;
    }
    service->release_worker();
    return false;
#else
    try {
        std::thread([service, tunnel, stream_id, stream, start_gate]() {
            PortTunnelWorkerLease lease(service);
            if (start_gate.get() != NULL) {
                start_gate->wait();
            }
            tunnel->tcp_read_loop(stream_id, stream);
        }).detach();
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn tcp read thread", ex);
        service->release_worker();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn tcp read thread");
        service->release_worker();
        return false;
    }
    return true;
#endif
}

bool spawn_tcp_write_thread(
    const std::shared_ptr<PortTunnelService>& service,
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream,
    bool worker_acquired
) {
    if (!worker_acquired && !service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    std::unique_ptr<TcpWriteContext> context(new TcpWriteContext());
    context->service = service;
    context->tunnel = tunnel;
    context->stream_id = stream_id;
    context->stream = stream;
    HANDLE handle = begin_win32_thread(tcp_write_thread_entry, context.get());
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
        return true;
    }
    service->release_worker();
    return false;
#else
    try {
        std::thread([service, tunnel, stream_id, stream]() {
            PortTunnelWorkerLease lease(service);
            tunnel->tcp_write_loop(stream_id, stream);
        }).detach();
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn tcp write thread", ex);
        service->release_worker();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn tcp write thread");
        service->release_worker();
        return false;
    }
    return true;
#endif
}

#ifdef _WIN32
struct UdpReadContext {
    std::shared_ptr<PortTunnelService> service;
    std::shared_ptr<PortTunnelConnection> tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelUdpSocket> socket_value;
};

unsigned __stdcall udp_read_thread_entry(void* raw_context) {
    std::unique_ptr<UdpReadContext> context(static_cast<UdpReadContext*>(raw_context));
    PortTunnelWorkerLease lease(context->service);
    context->tunnel->udp_read_loop_transport_owned(context->stream_id, context->socket_value);
    return 0;
}
#endif

bool spawn_udp_read_thread(
    const std::shared_ptr<PortTunnelService>& service,
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value,
    bool worker_acquired
) {
    if (!worker_acquired && !service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    std::unique_ptr<UdpReadContext> context(new UdpReadContext());
    context->service = service;
    context->tunnel = tunnel;
    context->stream_id = stream_id;
    context->socket_value = socket_value;
    HANDLE handle = begin_win32_thread(udp_read_thread_entry, context.get());
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
        return true;
    }
    service->release_worker();
    return false;
#else
    try {
        std::thread([service, tunnel, stream_id, socket_value]() {
            PortTunnelWorkerLease lease(service);
            tunnel->udp_read_loop_transport_owned(stream_id, socket_value);
        }).detach();
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn udp read thread", ex);
        service->release_worker();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn udp read thread");
        service->release_worker();
        return false;
    }
    return true;
#endif
}

int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        HttpResponse response;
        write_bearer_auth_challenge(response);
        send_all(client, render_http_response(response));
        return response.status;
    }
    if (request.method != "POST" || request.path != "/v1/port/tunnel" ||
        !connection_header_has_upgrade(request) ||
        header_token_lower(request, "upgrade") != "remote-exec-port-tunnel" ||
        request.header("x-remote-exec-port-tunnel-version") != "4") {
        HttpResponse response;
        write_rpc_error(response, 400, "bad_request", "invalid port tunnel upgrade request");
        send_all(client, render_http_response(response));
        return response.status;
    }

    send_all(
        client,
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: remote-exec-port-tunnel\r\n"
        "\r\n"
    );
    if (!state.port_tunnel_service) {
        state.port_tunnel_service =
            create_port_tunnel_service(state.config.port_forward_limits);
    }
    set_socket_timeout_ms(client, state.config.port_forward_limits.tunnel_io_timeout_ms);
    std::shared_ptr<PortTunnelConnection> tunnel(
        new PortTunnelConnection(client, state.port_tunnel_service)
    );
    tunnel->run();
    return 101;
}

bool PortTunnelConnection::read_exact(unsigned char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int ready = wait_socket_readable(client_, service_->limits().tunnel_io_timeout_ms);
        if (ready <= 0) {
            mark_closed();
            shutdown_socket(client_);
            return false;
        }
        const int received = recv_bounded(
            client_,
            reinterpret_cast<char*>(data + offset),
            size - offset,
            0
        );
        if (received == 0) {
            return false;
        }
        if (received < 0) {
            return false;
        }
        offset += static_cast<std::size_t>(received);
    }
    return true;
}

bool PortTunnelConnection::read_preface() {
    std::vector<unsigned char> bytes(port_tunnel_preface_size(), 0U);
    if (!read_exact(bytes.data(), bytes.size())) {
        return false;
    }
    return std::string(reinterpret_cast<const char*>(bytes.data()), bytes.size()) ==
           std::string(port_tunnel_preface(), port_tunnel_preface_size());
}

bool PortTunnelConnection::read_frame(PortTunnelFrame* frame) {
    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    if (!read_exact(bytes.data(), bytes.size())) {
        return false;
    }
    const uint32_t meta_len = (static_cast<uint32_t>(bytes[8]) << 24) |
                              (static_cast<uint32_t>(bytes[9]) << 16) |
                              (static_cast<uint32_t>(bytes[10]) << 8) |
                              static_cast<uint32_t>(bytes[11]);
    const uint32_t data_len = (static_cast<uint32_t>(bytes[12]) << 24) |
                              (static_cast<uint32_t>(bytes[13]) << 16) |
                              (static_cast<uint32_t>(bytes[14]) << 8) |
                              static_cast<uint32_t>(bytes[15]);
    if (meta_len > PORT_TUNNEL_MAX_META_LEN || data_len > PORT_TUNNEL_MAX_DATA_LEN) {
        throw PortTunnelFrameError("port tunnel frame exceeds maximum length");
    }
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U &&
        !read_exact(bytes.data() + PORT_TUNNEL_HEADER_LEN, meta_len + data_len)) {
        return false;
    }
    *frame = decode_port_tunnel_frame(bytes);
    return true;
}

PortTunnelSender::PortTunnelSender(
    SOCKET client,
    const std::shared_ptr<PortTunnelService>& service
) : client_(client),
    service_(service),
    writer_started_(false),
    writer_shutdown_(false),
    writer_finished_(false),
    closed_(false),
    queued_bytes_(0UL) {}

bool PortTunnelSender::closed() const {
    return closed_.load();
}

void PortTunnelSender::mark_closed() {
    closed_.store(true);
    BasicLockGuard lock(writer_mutex_);
    writer_shutdown_ = true;
    if (!writer_started_ || writer_finished_) {
        drain_queued_frame_reservations_locked();
        writer_cond_.broadcast();
        return;
    }
    writer_cond_.broadcast();
    while (!writer_finished_) {
        writer_cond_.wait(writer_mutex_);
    }
}

bool PortTunnelSender::ensure_writer_started_locked() {
    if (writer_started_) {
        return true;
    }
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelSender> sender;
    };
    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->sender->writer_loop();
            return 0;
        }
    };
    std::unique_ptr<Context> context(new Context());
    context->sender = shared_from_this();
    HANDLE handle = begin_win32_thread(&ThreadEntry::entry, context.get());
    if (handle == NULL) {
        closed_.store(true);
        writer_shutdown_ = true;
        drain_queued_frame_reservations_locked();
        shutdown_socket(client_);
        return false;
    }
    context.release();
    CloseHandle(handle);
    writer_started_ = true;
    return true;
#else
    try {
        std::shared_ptr<PortTunnelSender> self = shared_from_this();
        std::thread([self]() { self->writer_loop(); }).detach();
        writer_started_ = true;
        return true;
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn tunnel writer thread", ex);
        closed_.store(true);
        writer_shutdown_ = true;
        drain_queued_frame_reservations_locked();
        shutdown_socket(client_);
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn tunnel writer thread");
        closed_.store(true);
        writer_shutdown_ = true;
        drain_queued_frame_reservations_locked();
        shutdown_socket(client_);
        return false;
    }
#endif
}

void PortTunnelSender::writer_loop() {
    for (;;) {
        QueuedFrame queued;
        {
            BasicLockGuard lock(writer_mutex_);
            while (writer_queue_.empty() && !writer_shutdown_) {
                writer_cond_.wait(writer_mutex_);
            }
            if (writer_queue_.empty()) {
                writer_finished_ = true;
                writer_cond_.broadcast();
                return;
            }
            queued = std::move(writer_queue_.front());
            writer_queue_.pop_front();
        }

        try {
            send_all_bytes(
                client_,
                reinterpret_cast<const char*>(queued.bytes.data()),
                queued.bytes.size()
            );
        } catch (const std::exception& ex) {
            log_tunnel_exception("send port tunnel frame", ex);
            release_queued_frame_reservation(queued.charge_value);
            closed_.store(true);
            shutdown_socket(client_);
            {
                BasicLockGuard lock(writer_mutex_);
                writer_shutdown_ = true;
                drain_queued_frame_reservations_locked();
                writer_finished_ = true;
                writer_cond_.broadcast();
            }
            return;
        }
        release_queued_frame_reservation(queued.charge_value);
    }
}

bool PortTunnelSender::enqueue_encoded_frame(
    std::vector<unsigned char> bytes,
    unsigned long charge_value
) {
    BasicLockGuard lock(writer_mutex_);
    if (closed_.load() || writer_shutdown_) {
        return false;
    }
    if (!ensure_writer_started_locked()) {
        return false;
    }
    writer_queue_.push_back(QueuedFrame(std::move(bytes), charge_value));
    writer_cond_.signal();
    return true;
}

void PortTunnelSender::send_frame(const PortTunnelFrame& frame) {
    std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
    (void)enqueue_encoded_frame(std::move(bytes), 0UL);
}

bool PortTunnelSender::try_reserve_data_frame(
    const PortTunnelFrame& frame,
    unsigned long* charge_value
) {
    const std::size_t charge =
        PORT_TUNNEL_HEADER_LEN + frame.meta.size() + frame.data.size();
    if (charge > static_cast<std::size_t>(std::numeric_limits<unsigned long>::max())) {
        return false;
    }
    *charge_value = static_cast<unsigned long>(charge);
    const unsigned long limit = service_->limits().max_tunnel_queued_bytes;
    if (*charge_value > limit) {
        return false;
    }
    unsigned long current = queued_bytes_.load();
    for (;;) {
        if (current > limit || current > limit - *charge_value) {
            return false;
        }
        if (queued_bytes_.compare_exchange_weak(
                current,
                current + *charge_value
            )) {
            break;
        }
    }
    return true;
}

void PortTunnelSender::release_data_frame_reservation(unsigned long charge_value) {
    queued_bytes_.fetch_sub(charge_value);
}

void PortTunnelSender::release_queued_frame_reservation(unsigned long charge_value) {
    if (charge_value != 0UL) {
        release_data_frame_reservation(charge_value);
    }
}

void PortTunnelSender::drain_queued_frame_reservations_locked() {
    for (std::deque<QueuedFrame>::iterator it = writer_queue_.begin();
         it != writer_queue_.end();
         ++it) {
        release_queued_frame_reservation(it->charge_value);
    }
    writer_queue_.clear();
}

bool PortTunnelSender::send_data_frame_or_limit_error(
    PortTunnelConnection& connection,
    const PortTunnelFrame& frame
) {
    unsigned long charge_value = 0UL;
    if (!try_reserve_data_frame(frame, &charge_value)) {
        connection.send_error(
            frame.stream_id,
            "port_tunnel_limit_exceeded",
            "port tunnel queued byte limit reached"
        );
        return false;
    }
    try {
        std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        if (enqueue_encoded_frame(std::move(bytes), charge_value)) {
            return true;
        }
    } catch (const std::exception& ex) {
        log_tunnel_exception("queue limited port tunnel data frame", ex);
        release_data_frame_reservation(charge_value);
        throw;
    } catch (...) {
        log_unknown_tunnel_exception("queue limited port tunnel data frame");
        release_data_frame_reservation(charge_value);
        throw;
    }
    release_data_frame_reservation(charge_value);
    return false;
}

bool PortTunnelSender::send_data_frame_or_drop_on_limit(
    PortTunnelConnection& connection,
    const PortTunnelFrame& frame
) {
    unsigned long charge_value = 0UL;
    if (!try_reserve_data_frame(frame, &charge_value)) {
        if (frame.type == PortTunnelFrameType::UdpDatagram) {
            connection.send_forward_drop(
                frame.stream_id,
                "udp_datagram",
                "port_tunnel_limit_exceeded",
                "port tunnel queued byte limit reached"
            );
        }
        return true;
    }
    try {
        std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        if (enqueue_encoded_frame(std::move(bytes), charge_value)) {
            return true;
        }
    } catch (const std::exception& ex) {
        log_tunnel_exception("queue droppable port tunnel data frame", ex);
        release_data_frame_reservation(charge_value);
        throw;
    } catch (...) {
        log_unknown_tunnel_exception("queue droppable port tunnel data frame");
        release_data_frame_reservation(charge_value);
        throw;
    }
    release_data_frame_reservation(charge_value);
    return false;
}

void PortTunnelConnection::send_frame(const PortTunnelFrame& frame) {
    sender_->send_frame(frame);
}

bool PortTunnelConnection::send_data_frame_or_limit_error(const PortTunnelFrame& frame) {
    return sender_->send_data_frame_or_limit_error(*this, frame);
}

bool PortTunnelConnection::send_data_frame_or_drop_on_limit(const PortTunnelFrame& frame) {
    return sender_->send_data_frame_or_drop_on_limit(*this, frame);
}

bool PortTunnelConnection::closed() const {
    return sender_->closed();
}

void PortTunnelConnection::mark_closed() {
    sender_->mark_closed();
}

void TransportOwnedStreams::insert_tcp(
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream
) {
    BasicLockGuard lock(mutex_);
    tcp_streams_[stream_id] = stream;
}

std::shared_ptr<TunnelTcpStream> TransportOwnedStreams::get_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
        tcp_streams_.find(stream_id);
    if (it == tcp_streams_.end()) {
        return std::shared_ptr<TunnelTcpStream>();
    }
    return it->second;
}

std::shared_ptr<TunnelTcpStream> TransportOwnedStreams::remove_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
        tcp_streams_.find(stream_id);
    if (it == tcp_streams_.end()) {
        return std::shared_ptr<TunnelTcpStream>();
    }
    std::shared_ptr<TunnelTcpStream> stream = it->second;
    tcp_streams_.erase(it);
    return stream;
}

void TransportOwnedStreams::insert_udp(
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
    BasicLockGuard lock(mutex_);
    udp_sockets_[stream_id] = socket_value;
}

std::shared_ptr<TunnelUdpSocket> TransportOwnedStreams::get_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
        udp_sockets_.find(stream_id);
    if (it == udp_sockets_.end()) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    return it->second;
}

std::shared_ptr<TunnelUdpSocket> TransportOwnedStreams::remove_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
        udp_sockets_.find(stream_id);
    if (it == udp_sockets_.end()) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    std::shared_ptr<TunnelUdpSocket> socket_value = it->second;
    udp_sockets_.erase(it);
    return socket_value;
}

void TransportOwnedStreams::drain(
    std::vector<std::shared_ptr<TunnelTcpStream> >* tcp_streams,
    std::vector<std::shared_ptr<TunnelUdpSocket> >* udp_sockets
) {
    BasicLockGuard lock(mutex_);
    for (std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
             tcp_streams_.begin();
         it != tcp_streams_.end();
         ++it) {
        tcp_streams->push_back(it->second);
    }
    tcp_streams_.clear();
    for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
             udp_sockets_.begin();
         it != udp_sockets_.end();
         ++it) {
        udp_sockets->push_back(it->second);
    }
    udp_sockets_.clear();
}

void PortTunnelConnection::run() {
    if (!read_preface()) {
        return;
    }

    PortTunnelCloseMode close_mode = PortTunnelCloseMode::RetryableDetach;
    try {
        for (;;) {
            PortTunnelFrame frame;
            if (!read_frame(&frame)) {
                break;
            }
            handle_frame(frame);
        }
    } catch (const std::exception& ex) {
        close_mode = PortTunnelCloseMode::TerminalFailure;
        send_terminal_error(0U, "invalid_port_tunnel", ex.what());
    } catch (...) {
        close_mode = PortTunnelCloseMode::TerminalFailure;
        send_terminal_error(
            0U, "invalid_port_tunnel", "unknown port tunnel failure"
        );
    }
    close_current_session(close_mode);
    close_transport_owned_state();
}

void PortTunnelConnection::handle_frame(const PortTunnelFrame& frame) {
    try {
        switch (frame.type) {
        case PortTunnelFrameType::TunnelOpen:
            tunnel_open(frame);
            break;
        case PortTunnelFrameType::TunnelClose:
            tunnel_close(frame);
            break;
        case PortTunnelFrameType::TunnelHeartbeat:
            tunnel_heartbeat(frame);
            break;
        case PortTunnelFrameType::TcpListen:
            tcp_listen(frame);
            break;
        case PortTunnelFrameType::TcpConnect:
            tcp_connect(frame);
            break;
        case PortTunnelFrameType::TcpData:
            tcp_data(frame.stream_id, frame.data);
            break;
        case PortTunnelFrameType::TcpEof:
            tcp_eof(frame.stream_id);
            break;
        case PortTunnelFrameType::UdpBind:
            udp_bind(frame);
            break;
        case PortTunnelFrameType::UdpDatagram:
            udp_datagram(frame);
            break;
        case PortTunnelFrameType::Close:
            close_stream(frame.stream_id);
            break;
        default:
            send_error(frame.stream_id, "invalid_port_tunnel", "unexpected frame from broker");
            break;
        }
    } catch (const PortForwardError& ex) {
        send_error(frame.stream_id, ex.code(), ex.what());
    } catch (const std::exception& ex) {
        send_error(frame.stream_id, "internal_error", ex.what());
    }
}

void PortTunnelConnection::tunnel_open(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "tunnel open must use stream_id 0");
    }
    {
        BasicLockGuard lock(state_mutex_);
        if (mode_ != PortTunnelMode::Unopened || session_.get() != NULL) {
            throw PortForwardError(
                400,
                "port_tunnel_already_attached",
                "port tunnel is already open"
            );
        }
    }

    const Json meta = Json::parse(frame.meta);
    const std::string role = meta.at("role").get<std::string>();
    const std::uint64_t generation = meta.at("generation").get<std::uint64_t>();
    const std::string protocol = meta.at("protocol").get<std::string>();
    PortTunnelProtocol tunnel_protocol = PortTunnelProtocol::None;
    if (protocol == "tcp") {
        tunnel_protocol = PortTunnelProtocol::Tcp;
    } else if (protocol == "udp") {
        tunnel_protocol = PortTunnelProtocol::Udp;
    } else {
        throw PortForwardError(400, "invalid_port_tunnel", "unknown tunnel protocol");
    }
    set_generation(generation);

    if (role == "listen") {
        std::shared_ptr<PortTunnelSession> session;
        if (meta.contains("resume_session_id") && !meta.at("resume_session_id").is_null()) {
            const std::string session_id = meta.at("resume_session_id").get<std::string>();
            session = service_->find_session(session_id);
            if (session.get() == NULL) {
                throw PortForwardError(
                    400,
                    "unknown_port_tunnel_session",
                    "unknown port tunnel session"
                );
            }

            bool expired = false;
            {
                BasicLockGuard lock(session->mutex);
                if (session->closed) {
                    throw PortForwardError(
                        400,
                        "unknown_port_tunnel_session",
                        "unknown port tunnel session"
                    );
                }
                if (session->attached) {
                    throw PortForwardError(
                        400,
                        "port_tunnel_already_attached",
                        "port tunnel session is already attached"
                    );
                }
                expired = session->expired ||
                          (session->resume_deadline_ms != 0ULL &&
                           platform::monotonic_ms() >= session->resume_deadline_ms);
                session->generation = generation;
            }
            if (expired) {
                service_->close_session(session);
                throw PortForwardError(
                    400,
                    "port_tunnel_resume_expired",
                    "port tunnel resume expired"
                );
            }
        } else {
            session = service_->create_session();
            {
                BasicLockGuard lock(session->mutex);
                session->generation = generation;
            }
        }

        {
            BasicLockGuard lock(state_mutex_);
            session_ = session;
            mode_ = PortTunnelMode::Listen;
            protocol_ = tunnel_protocol;
        }
        service_->attach_session(session, shared_from_this());

        const PortForwardLimitConfig& limits = service_->limits();
        PortTunnelFrame ready = make_empty_frame(PortTunnelFrameType::TunnelReady, 0U);
        ready.meta = Json{
            {"generation", generation},
            {"session_id", session->session_id},
            {"resume_timeout_ms", RESUME_TIMEOUT_MS},
            {"limits", Json{
                {"max_active_tcp_streams", limits.max_active_tcp_streams},
                {"max_udp_peers", limits.max_udp_binds},
                {"max_queued_bytes", limits.max_tunnel_queued_bytes}
            }}
        }.dump();
        send_frame(ready);
        return;
    }

    if (role == "connect") {
        {
            BasicLockGuard lock(state_mutex_);
            mode_ = PortTunnelMode::Connect;
            protocol_ = tunnel_protocol;
        }
        const PortForwardLimitConfig& limits = service_->limits();
        PortTunnelFrame ready = make_empty_frame(PortTunnelFrameType::TunnelReady, 0U);
        ready.meta = Json{
            {"generation", generation},
            {"limits", Json{
                {"max_active_tcp_streams", limits.max_active_tcp_streams},
                {"max_udp_peers", limits.max_udp_binds},
                {"max_queued_bytes", limits.max_tunnel_queued_bytes}
            }}
        }.dump();
        send_frame(ready);
        return;
    }

    throw PortForwardError(400, "invalid_port_tunnel", "unknown tunnel role");
}

void PortTunnelConnection::tunnel_close(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "tunnel close must use stream_id 0");
    }
    const Json meta = Json::parse(frame.meta);
    ensure_generation(meta.at("generation").get<std::uint64_t>());
    PortTunnelFrame closed = make_empty_frame(PortTunnelFrameType::TunnelClosed, 0U);
    closed.meta = frame.meta;
    send_frame(closed);
    close_current_session(PortTunnelCloseMode::GracefulClose);
    close_transport_owned_state();
}

void PortTunnelConnection::tunnel_heartbeat(const PortTunnelFrame& frame) {
    PortTunnelFrame ack = make_empty_frame(PortTunnelFrameType::TunnelHeartbeatAck, 0U);
    ack.meta = frame.meta;
    send_frame(ack);
}
