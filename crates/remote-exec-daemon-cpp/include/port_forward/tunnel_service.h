#pragma once

#include <memory>
#include <string>

#include "core/config.h"
#include "core/stream.h"
#include "http/http_helpers.h"

class EventLoop;
class BlockingPool;
struct TunnelServiceImpl;

// Loop-owned v4 port tunnel service. Listen sessions retain their listener or
// UDP bind across transport loss and can be resumed by session id. All sockets
// in this subsystem belong to the service loop.
class TunnelService {
public:
    TunnelService(
        const PortForwardLimitConfig& limits,
        const std::string& bearer_token,
        EventLoop* loop,
        BlockingPool* pool
    );
    ~TunnelService();

    bool validate_upgrade(const HttpRequest& request, HttpResponse* error) const;
    void adopt(const std::shared_ptr<Stream>& transport, const std::string& initial_input);
    void tick();
    void shutdown();

    TunnelService(const TunnelService&) = delete;
    TunnelService& operator=(const TunnelService&) = delete;

private:
    std::unique_ptr<TunnelServiceImpl> impl_;
};
