#pragma once

#include <atomic>
#include <cstdint>
#include <map>
#include <memory>
#include <string>
#include <thread>
#include <vector>

#include "core/config.h"
#include "platform/basic_mutex.h"
#include "platform/socket.h"
#include "runtime/app_state.h"

class EventLoop;
class BlockingPool;
class ReactorRouter;
class HttpConn;

// Reactor-based daemon core. The listener lives on shard 0 and hands accepted
// connections to the shard that will own them. On Winsock 1.1 each shard is a
// select() set, so multiple shards are how the daemon exceeds FD_SETSIZE.
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
    bool assign_shard(std::size_t* shard);
    std::size_t shard_capacity(std::size_t shard) const;
    void resume_listener();

    DaemonConfig config_;
    AppMetadata metadata_;
    AppSandboxState sandbox_;
    std::vector<std::unique_ptr<EventLoop>> loops_;
    std::vector<std::thread> loop_threads_;
    std::unique_ptr<BlockingPool> pool_;
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
};
