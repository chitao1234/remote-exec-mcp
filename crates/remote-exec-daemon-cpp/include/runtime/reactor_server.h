#pragma once

#include <atomic>
#include <cstdint>
#include <map>
#include <memory>
#include <string>
#include <thread>
#include <vector>

#include "core/config.h"
#include "core/stream.h"
#include "platform/basic_mutex.h"
#include "platform/socket.h"
#include "runtime/app_state.h"

class EventLoop;
class BlockingPool;
class ExecService;
class ReactorRouter;
class TunnelService;
class TlsContext;
class HttpConn;
struct ReverseConn;

// Reactor-based daemon core. The listener lives on shard 0 and hands accepted
// connections to the shard that will own them. On Winsock 1.1 each shard uses
// select() sets, and the tunnel runs on a dedicated loop so forwarded sockets
// do not compete with HTTP connections for a shard.
class ReactorServer {
public:
    explicit ReactorServer(const DaemonConfig& config);
    ~ReactorServer();

    bool start(std::string* error);
    void run();
    void request_stop();

    unsigned short bound_port() const;
    const DaemonConfig& config() const;
    const AppMetadata& metadata() const;
    std::size_t shard_count() const;

    ReactorServer(const ReactorServer&) = delete;
    ReactorServer& operator=(const ReactorServer&) = delete;

private:
    void accept_ready();
    void on_conn_closed(std::uint64_t id);
    void stop_loops();
    void finalize();
    void schedule_exec_tick();
    void exec_tick();
    void schedule_tunnel_tick();
    void tunnel_tick();
    void reverse_tick();
    void start_reverse_connect();
    void begin_reverse_connect(const std::vector<SocketAddress>& addresses);
    void serve_reverse_connection(const std::shared_ptr<Stream>& stream);
    void reverse_attempt_finished(bool success);
    bool assign_shard(std::size_t* shard);
    std::size_t shard_capacity(std::size_t shard) const;
    void resume_listener();

    DaemonConfig config_;
    AppMetadata metadata_;
    AppSandboxState sandbox_;
    std::vector<std::unique_ptr<EventLoop>> loops_;
    std::vector<std::thread> loop_threads_;
    std::unique_ptr<EventLoop> tunnel_loop_;
    std::thread tunnel_thread_;
    std::unique_ptr<BlockingPool> pool_;
    friend struct ReverseConn;

    std::shared_ptr<TlsContext> tls_server_context_;
    std::shared_ptr<TlsContext> tls_client_context_;
    std::unique_ptr<ExecService> exec_;
    std::unique_ptr<TunnelService> tunnel_;
    std::unique_ptr<ReactorRouter> router_;
    UniqueSocket listener_;
    BasicMutex connections_mutex_;
    std::map<std::uint64_t, std::shared_ptr<HttpConn>> connections_;
    std::vector<std::size_t> shard_connections_;
    std::size_t next_shard_;
    std::uint64_t next_connection_id_;
    unsigned short bound_port_;
    bool listener_paused_;
    std::atomic<bool> shutting_down_;
    bool finalized_;
    std::atomic<unsigned long> reverse_live_;
    std::atomic<unsigned long> reverse_busy_;
    unsigned long reverse_max_;
    bool reverse_connect_in_progress_;
    std::uint64_t reverse_next_attempt_ms_;
    std::uint64_t next_reverse_id_;
};
