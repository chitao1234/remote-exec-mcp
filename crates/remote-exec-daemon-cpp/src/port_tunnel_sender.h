#pragma once

#include <atomic>
#include <deque>
#include <memory>
#include <utility>
#include <vector>
#ifndef _WIN32
#include <thread>
#endif

#include "basic_mutex.h"
#include "port_tunnel_common.h"

class PortTunnelConnection;
class PortTunnelService;

class PortTunnelSender {
public:
    PortTunnelSender(SOCKET client, const std::shared_ptr<PortTunnelService>& service);
    ~PortTunnelSender();

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
    void join_writer_thread();
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
#ifdef _WIN32
    HANDLE writer_thread_;
    DWORD writer_thread_id_;
#else
    std::unique_ptr<std::thread> writer_thread_;
#endif
    std::atomic<bool> closed_;
    std::atomic<unsigned long> queued_bytes_;
};
