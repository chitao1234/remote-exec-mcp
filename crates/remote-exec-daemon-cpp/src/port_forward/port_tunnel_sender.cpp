#include "port_tunnel_sender.h"

#include <limits>

#include "../daemon_thread.h"
#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"

namespace {

// Data queues can be configured very small for pressure tests; control/error
// frames need an independent floor so they can still report those limits.
const unsigned long MIN_CONTROL_QUEUE_BYTES = 4UL * 1024UL;

} // namespace

PortTunnelSender::PortTunnelSender(SOCKET client, const std::shared_ptr<PortTunnelService>& service)
    : client_(client), service_(service), writer_started_(false), writer_shutdown_(false), writer_finished_(false),
      writer_thread_(
#ifdef _WIN32
          nullptr
#endif
          ),
#ifdef _WIN32
      writer_thread_id_(0U),
#endif
      closed_(false), queued_data_bytes_(0UL), queued_control_bytes_(0UL) {
}

PortTunnelSender::~PortTunnelSender() {
    mark_closed();
}

bool PortTunnelSender::closed() const {
    return closed_.load();
}

void PortTunnelSender::mark_closed() {
    {
        BasicLockGuard lock(writer_mutex_);
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("sender close requested")
                        .bool_field("writer_started", writer_started_)
                        .bool_field("writer_finished", writer_finished_)
                        .field("queued_data_bytes", queued_data_bytes_.load())
                        .field("queued_control_bytes", queued_control_bytes_.load())
                        .str());
        closed_.store(true);
        writer_shutdown_ = true;
        if (!writer_started_ || writer_finished_) {
            drain_queued_frame_reservations_locked();
            writer_cond_.broadcast();
        } else {
            writer_cond_.broadcast();
            while (!writer_finished_) {
                writer_cond_.wait(writer_mutex_);
            }
        }
    }
    join_writer_thread();
}

void PortTunnelSender::join_writer_thread() {
#ifdef _WIN32
    HANDLE thread = nullptr;
    DWORD thread_id = 0U;
    {
        BasicLockGuard lock(writer_mutex_);
        thread = writer_thread_;
        thread_id = writer_thread_id_;
        writer_thread_ = nullptr;
        writer_thread_id_ = 0U;
    }
    consume_daemon_thread(&thread, thread_id);
#else
    std::unique_ptr<std::thread> thread;
    {
        BasicLockGuard lock(writer_mutex_);
        thread.swap(writer_thread_);
    }
    consume_daemon_thread(&thread);
#endif
}

bool PortTunnelSender::ensure_writer_started_locked() {
    if (writer_started_) {
        return true;
    }
#ifdef _WIN32
    struct Context {
        PortTunnelSender* sender;
    };
    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            log_message(LOG_DEBUG, "port_tunnel", "sender writer start");
            context->sender->writer_loop();
            log_message(LOG_DEBUG, "port_tunnel", "sender writer stop");
            return 0;
        }
    };
    std::unique_ptr<Context> context(new Context());
    context->sender = this;
    HANDLE handle = begin_win32_thread(&ThreadEntry::entry, context.get());
    if (handle == nullptr) {
        closed_.store(true);
        writer_shutdown_ = true;
        drain_queued_frame_reservations_locked();
        writer_cond_.broadcast();
        return false;
    }
    writer_thread_ = handle;
    context.release();
    writer_started_ = true;
    return true;
#else
    try {
        writer_thread_.reset(new std::thread([this]() {
            log_message(LOG_DEBUG, "port_tunnel", "sender writer start");
            writer_loop();
            log_message(LOG_DEBUG, "port_tunnel", "sender writer stop");
        }));
        writer_started_ = true;
        return true;
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn tunnel writer thread", ex);
        closed_.store(true);
        writer_shutdown_ = true;
        drain_queued_frame_reservations_locked();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn tunnel writer thread");
        closed_.store(true);
        writer_shutdown_ = true;
        drain_queued_frame_reservations_locked();
        return false;
    }
#endif
}

void PortTunnelSender::writer_loop() {
#ifdef _WIN32
    {
        BasicLockGuard lock(writer_mutex_);
        writer_thread_id_ = GetCurrentThreadId();
    }
#endif
    for (;;) {
        QueuedFrame queued;
        {
            BasicLockGuard lock(writer_mutex_);
            while (writer_queue_.empty() && !writer_shutdown_) {
                writer_cond_.wait(writer_mutex_);
            }
            if (writer_queue_.empty()) {
                log_message(LOG_DEBUG, "port_tunnel", "sender writer drained");
                writer_finished_ = true;
                writer_cond_.broadcast();
                return;
            }
            queued = std::move(writer_queue_.front());
            writer_queue_.pop_front();
        }

        try {
            send_all_bytes(client_, reinterpret_cast<const char*>(queued.bytes.data()), queued.bytes.size());
        } catch (const std::exception& ex) {
            log_tunnel_exception("send port tunnel frame", ex);
            release_queued_frame_reservation(queued.charge_kind, queued.charge_value);
            closed_.store(true);
            {
                BasicLockGuard lock(writer_mutex_);
                writer_shutdown_ = true;
                drain_queued_frame_reservations_locked();
                writer_finished_ = true;
                writer_cond_.broadcast();
            }
            return;
        }
        release_queued_frame_reservation(queued.charge_kind, queued.charge_value);
    }
}

bool PortTunnelSender::enqueue_encoded_frame(std::vector<unsigned char> bytes,
                                             QueueReservationKind charge_kind,
                                             unsigned long charge_value) {
    BasicLockGuard lock(writer_mutex_);
    if (closed_.load() || writer_shutdown_) {
        return false;
    }
    if (!ensure_writer_started_locked()) {
        return false;
    }
    writer_queue_.push_back(QueuedFrame(std::move(bytes), charge_kind, charge_value));
    writer_cond_.signal();
    return true;
}

bool PortTunnelSender::send_frame(const PortTunnelFrame& frame) {
    std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
    unsigned long charge_value = 0UL;
    if (!try_reserve_control_frame(bytes, &charge_value)) {
        return false;
    }
    if (enqueue_encoded_frame(std::move(bytes), QueueReservationKind::Control, charge_value)) {
        return true;
    }
    release_control_frame_reservation(charge_value);
    return false;
}

unsigned long PortTunnelSender::control_queue_byte_limit() const {
    const unsigned long configured = service_->limits().max_tunnel_queued_bytes;
    return configured > MIN_CONTROL_QUEUE_BYTES ? configured : MIN_CONTROL_QUEUE_BYTES;
}

bool PortTunnelSender::try_reserve_queued_bytes(std::atomic<unsigned long>& counter,
                                                unsigned long limit,
                                                unsigned long charge_value) {
    if (charge_value > limit) {
        return false;
    }
    unsigned long current = counter.load();
    for (;;) {
        if (current > limit || current > limit - charge_value) {
            return false;
        }
        if (counter.compare_exchange_weak(current, current + charge_value)) {
            break;
        }
    }
    return true;
}

bool PortTunnelSender::try_reserve_frame_bytes(std::size_t byte_count,
                                               std::atomic<unsigned long>& counter,
                                               unsigned long limit,
                                               unsigned long* charge_value) {
    if (byte_count > static_cast<std::size_t>(std::numeric_limits<unsigned long>::max())) {
        return false;
    }
    *charge_value = static_cast<unsigned long>(byte_count);
    return try_reserve_queued_bytes(counter, limit, *charge_value);
}

bool PortTunnelSender::try_reserve_data_frame(const PortTunnelFrame& frame, unsigned long* charge_value) {
    const std::size_t charge = PORT_TUNNEL_HEADER_LEN + frame.meta.size() + frame.data.size();
    return try_reserve_frame_bytes(charge, queued_data_bytes_, service_->limits().max_tunnel_queued_bytes, charge_value);
}

bool PortTunnelSender::try_reserve_control_frame(const std::vector<unsigned char>& bytes,
                                                 unsigned long* charge_value) {
    return try_reserve_frame_bytes(bytes.size(), queued_control_bytes_, control_queue_byte_limit(), charge_value);
}

void PortTunnelSender::release_data_frame_reservation(unsigned long charge_value) {
    queued_data_bytes_.fetch_sub(charge_value);
}

void PortTunnelSender::release_control_frame_reservation(unsigned long charge_value) {
    queued_control_bytes_.fetch_sub(charge_value);
}

void PortTunnelSender::release_queued_frame_reservation(QueueReservationKind charge_kind,
                                                        unsigned long charge_value) {
    if (charge_value == 0UL) {
        return;
    }
    if (charge_kind == QueueReservationKind::Data) {
        release_data_frame_reservation(charge_value);
    } else if (charge_kind == QueueReservationKind::Control) {
        release_control_frame_reservation(charge_value);
    }
}

void PortTunnelSender::drain_queued_frame_reservations_locked() {
    for (std::deque<QueuedFrame>::iterator it = writer_queue_.begin(); it != writer_queue_.end(); ++it) {
        release_queued_frame_reservation(it->charge_kind, it->charge_value);
    }
    writer_queue_.clear();
}

bool PortTunnelSender::send_data_frame_or_limit_error(PortTunnelConnection& connection, const PortTunnelFrame& frame) {
    unsigned long charge_value = 0UL;
    if (!try_reserve_data_frame(frame, &charge_value)) {
        connection.send_error(frame.stream_id, "port_tunnel_limit_exceeded", "port tunnel queued byte limit reached");
        return false;
    }
    try {
        std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        if (enqueue_encoded_frame(std::move(bytes), QueueReservationKind::Data, charge_value)) {
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

bool PortTunnelSender::send_data_frame_or_drop_on_limit(PortTunnelConnection& connection,
                                                        const PortTunnelFrame& frame) {
    unsigned long charge_value = 0UL;
    if (!try_reserve_data_frame(frame, &charge_value)) {
        if (frame.type == PortTunnelFrameType::UdpDatagram) {
            connection.send_forward_drop(
                frame.stream_id, "udp_datagram", "port_tunnel_limit_exceeded", "port tunnel queued byte limit reached");
        }
        return true;
    }
    try {
        std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        if (enqueue_encoded_frame(std::move(bytes), QueueReservationKind::Data, charge_value)) {
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
