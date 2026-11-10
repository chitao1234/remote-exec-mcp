#pragma once

#include <atomic>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>

#include "core/byte_buffer.h"
#include "core/event_loop.h"
#include "core/stream.h"
#include "http/http_codec.h"
#include "http/http_helpers.h"
#include "platform/socket.h"

class BlockingPool;
class ExecService;
class ReactorRouter;

// One HTTP/1.1 connection driven entirely by loop readiness. The connection
// never blocks: reads and writes return WouldBlock and interest is flipped.
class HttpConn : public std::enable_shared_from_this<HttpConn> {
public:
    struct Options {
        EventLoop* loop;
        BlockingPool* pool;
        const ReactorRouter* router;
        ExecService* exec;
        std::size_t max_header_bytes;
        std::size_t max_body_bytes;
        unsigned long idle_timeout_ms;
        std::atomic<bool>* shutting_down;
        std::function<void(std::uint64_t)> on_closed;
    };

    HttpConn(
        std::uint64_t id,
        std::size_t shard_index,
        UniqueSocket socket,
        const Options& options
    );
    ~HttpConn();

    void start();
    void close();
    bool closed() const;
    std::size_t shard_index() const;
    void finish_handler(const HttpResponse& response);

private:
    enum class State {
        Reading,
        WaitingHandler,
        Writing,
        Closed,
    };

    struct ChunkScanner {
        ChunkScanner();

        void reset(std::size_t start, std::size_t body_limit);
        bool feed(const char* data, std::size_t size);
        bool failed() const;

        int state;
        std::size_t cursor;
        std::size_t remaining;
        std::size_t decoded;
        std::size_t limit;
        std::size_t end_offset;
    };

    void on_events(int bits);
    void on_readable();
    void on_writable();
    void on_handler_done(const std::shared_ptr<HttpResponse>& response);
    bool process_input();
    void dispatch(HttpRequest& request);
    void begin_write(const HttpResponse& response, bool keep_alive);
    void fail(int status, const char* code, const std::string& message);
    void arm_timer();
    void arm_timer_with(unsigned long timeout_ms);
    void cancel_timer();

    std::uint64_t id_;
    std::size_t shard_index_;
    SOCKET fd_;
    std::shared_ptr<Stream> stream_;
    EventLoop* loop_;
    BlockingPool* pool_;
    const ReactorRouter* router_;
    ExecService* exec_;
    std::size_t max_header_bytes_;
    std::size_t max_body_bytes_;
    unsigned long idle_timeout_ms_;
    std::atomic<bool>* shutting_down_;
    std::function<void(std::uint64_t)> on_closed_;

    ByteBuffer input_;
    std::string output_;
    std::size_t output_offset_;
    State state_;
    bool closed_;
    bool head_parsed_;
    bool keep_alive_;
    bool close_after_write_;
    std::size_t header_end_;
    std::size_t body_length_;
    HttpRequestBodyFraming framing_;
    HttpRequest head_;
    ChunkScanner scanner_;
    EventLoop::TimerId timer_;
    bool timer_armed_;
};
