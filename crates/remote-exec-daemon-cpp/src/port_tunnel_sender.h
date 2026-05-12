#pragma once

#include <atomic>
#include <deque>
#include <memory>
#include <utility>
#include <vector>

#include "basic_mutex.h"
#include "port_tunnel_common.h"

class PortTunnelConnection;
class PortTunnelService;

class PortTunnelSender : public std::enable_shared_from_this<PortTunnelSender> {
public:
    PortTunnelSender(SOCKET client, const std::shared_ptr<PortTunnelService>& service);

    bool closed() const;
    void mark_closed();
    void send_frame(const PortTunnelFrame& frame);
    bool send_data_frame_or_limit_error(PortTunnelConnection& connection, const PortTunnelFrame& frame);
    bool send_data_frame_or_drop_on_limit(PortTunnelConnection& connection, const PortTunnelFrame& frame);

private:
    PortTunnelSender(const PortTunnelSender&);
    PortTunnelSender& operator=(const PortTunnelSender&);

    struct QueuedFrame {
        QueuedFrame() : charge_value(0UL) {}
        QueuedFrame(std::vector<unsigned char> bytes_value, unsigned long charge)
            : bytes(std::move(bytes_value)), charge_value(charge) {}

        std::vector<unsigned char> bytes;
        unsigned long charge_value;
    };

    void writer_loop();
    bool ensure_writer_started_locked();
    bool enqueue_encoded_frame(std::vector<unsigned char> bytes, unsigned long charge_value);
    bool try_reserve_data_frame(const PortTunnelFrame& frame, unsigned long* charge_value);
    void release_data_frame_reservation(unsigned long charge_value);
    void release_queued_frame_reservation(unsigned long charge_value);
    void drain_queued_frame_reservations_locked();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    BasicMutex writer_mutex_;
    BasicCondVar writer_cond_;
    std::deque<QueuedFrame> writer_queue_;
    bool writer_started_;
    bool writer_shutdown_;
    bool writer_finished_;
    std::atomic<bool> closed_;
    std::atomic<unsigned long> queued_bytes_;
};
