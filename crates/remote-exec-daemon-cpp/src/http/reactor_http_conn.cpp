#include "http/reactor_http_conn.h"

#include <algorithm>
#include <cstring>
#include <utility>

#include "core/blocking_pool.h"
#include "core/logging.h"
#include "core/text_utils.h"
#include "exec/exec_service.h"
#include "http/http_request.h"
#include "platform/platform.h"
#include "port_forward/tunnel_service.h"
#include "rpc/reactor_router.h"
#include "rpc/server_contract.h"
#include "rpc/server_route_common.h"

namespace {

const std::size_t READ_CHUNK_SIZE = 16U * 1024U;
const int READ_CALLS_PER_EVENT = 8;
const std::size_t CHUNK_LINE_LIMIT = 8U * 1024U;
const std::size_t PIPELINE_ALLOWANCE = 64U * 1024U;
const unsigned long EXEC_REQUEST_SAFETY_MS = 330000UL;

enum ChunkState {
    CHUNK_SIZE = 0,
    CHUNK_DATA,
    CHUNK_DATA_END,
    CHUNK_TRAILERS,
    CHUNK_DONE,
    CHUNK_ERROR,
};

int hex_value(char ch) {
    if (ch >= '0' && ch <= '9') {
        return ch - '0';
    }
    if (ch >= 'a' && ch <= 'f') {
        return ch - 'a' + 10;
    }
    if (ch >= 'A' && ch <= 'F') {
        return ch - 'A' + 10;
    }
    return -1;
}

const char* find_crlf(const char* data, std::size_t size) {
    for (std::size_t i = 0; i + 1U < size; ++i) {
        if (data[i] == '\r' && data[i + 1U] == '\n') {
            return data + i;
        }
    }
    return nullptr;
}

bool connection_close_requested(const HttpRequest& request) {
    const std::string value = lowercase_ascii(request.header("connection"));
    std::size_t offset = 0U;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset)
        );
        if (token == "close") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }
    return false;
}

std::size_t input_limit_for(std::size_t max_header_bytes, std::size_t max_body_bytes) {
    const std::size_t max_size = static_cast<std::size_t>(-1);
    if (max_header_bytes > max_size - max_body_bytes) {
        return max_size;
    }
    const std::size_t combined = max_header_bytes + max_body_bytes;
    if (combined > max_size - PIPELINE_ALLOWANCE) {
        return combined;
    }
    return combined + PIPELINE_ALLOWANCE;
}

} // namespace

HttpConn::ChunkScanner::ChunkScanner()
    : state(CHUNK_SIZE), cursor(0U), remaining(0U), decoded(0U), limit(0U), end_offset(0U) {
}

void HttpConn::ChunkScanner::reset(std::size_t start, std::size_t body_limit) {
    state = CHUNK_SIZE;
    cursor = start;
    remaining = 0U;
    decoded = 0U;
    limit = body_limit;
    end_offset = start;
}

bool HttpConn::ChunkScanner::failed() const {
    return state == CHUNK_ERROR;
}

bool HttpConn::ChunkScanner::feed(const char* data, std::size_t size) {
    for (;;) {
        if (state == CHUNK_DONE || state == CHUNK_ERROR) {
            return true;
        }

        if (state == CHUNK_SIZE) {
            const char* crlf = find_crlf(data + cursor, size - cursor);
            if (crlf == nullptr) {
                if (size - cursor > CHUNK_LINE_LIMIT) {
                    state = CHUNK_ERROR;
                    return true;
                }
                return false;
            }
            const std::size_t line_length = static_cast<std::size_t>(crlf - (data + cursor));
            if (line_length == 0U) {
                state = CHUNK_ERROR;
                return true;
            }
            std::size_t value = 0U;
            std::size_t index = 0U;
            for (; index < line_length && data[cursor + index] != ';'; ++index) {
                const int digit = hex_value(data[cursor + index]);
                if (digit < 0) {
                    state = CHUNK_ERROR;
                    return true;
                }
                if (value > (limit - decoded) / 16U) {
                    state = CHUNK_ERROR;
                    return true;
                }
                value = value * 16U + static_cast<std::size_t>(digit);
                if (value > limit - decoded) {
                    state = CHUNK_ERROR;
                    return true;
                }
            }
            if (index == 0U) {
                state = CHUNK_ERROR;
                return true;
            }
            cursor += line_length + 2U;
            if (value == 0U) {
                state = CHUNK_TRAILERS;
            } else {
                remaining = value;
                state = CHUNK_DATA;
            }
            continue;
        }

        if (state == CHUNK_DATA) {
            const std::size_t available = size - cursor;
            const std::size_t take = std::min(available, remaining);
            if (take == 0U) {
                return false;
            }
            decoded += take;
            cursor += take;
            remaining -= take;
            if (remaining == 0U) {
                state = CHUNK_DATA_END;
            }
            continue;
        }

        if (state == CHUNK_DATA_END) {
            if (size - cursor < 2U) {
                return false;
            }
            if (data[cursor] != '\r' || data[cursor + 1U] != '\n') {
                state = CHUNK_ERROR;
                return true;
            }
            cursor += 2U;
            state = CHUNK_SIZE;
            continue;
        }

        // CHUNK_TRAILERS
        const char* crlf = find_crlf(data + cursor, size - cursor);
        if (crlf == nullptr) {
            if (size - cursor > CHUNK_LINE_LIMIT) {
                state = CHUNK_ERROR;
                return true;
            }
            return false;
        }
        const std::size_t line_length = static_cast<std::size_t>(crlf - (data + cursor));
        if (line_length == 0U) {
            cursor += 2U;
            end_offset = cursor;
            state = CHUNK_DONE;
            return true;
        }
        cursor += line_length + 2U;
    }
}

HttpConn::HttpConn(
    std::uint64_t id,
    std::size_t shard_index,
    std::shared_ptr<Stream> stream,
    const Options& options
)
    : id_(id), shard_index_(shard_index), fd_(stream ? stream->native_fd() : INVALID_SOCKET),
      stream_(std::move(stream)), loop_(options.loop), pool_(options.pool), router_(options.router),
      exec_(options.exec), tunnel_(options.tunnel), max_header_bytes_(options.max_header_bytes),
      max_body_bytes_(options.max_body_bytes), idle_timeout_ms_(options.idle_timeout_ms),
      shutting_down_(options.shutting_down), on_closed_(options.on_closed),
      input_(input_limit_for(options.max_header_bytes, options.max_body_bytes)), output_offset_(0U),
      state_(State::Reading), closed_(false), head_parsed_(false), first_request_(true),
      keep_alive_(true), close_after_write_(false), upgrade_after_write_(false), header_end_(0U),
      body_length_(0U), framing_(), head_(), scanner_(), timer_(0U), timer_armed_(false) {
}

HttpConn::~HttpConn() {
    if (stream_) {
        stream_->close();
    }
}

bool HttpConn::closed() const {
    return closed_;
}

std::size_t HttpConn::shard_index() const {
    return shard_index_;
}

void HttpConn::finish_handler(const HttpResponse& response) {
    if (closed_ || state_ != State::WaitingHandler) {
        return;
    }
    begin_write(response, keep_alive_);
}

void HttpConn::start() {
    if (closed_) {
        return;
    }
    std::shared_ptr<HttpConn> self = shared_from_this();
    if (!loop_->add(fd_, IO_READ, [self](int bits) { self->on_events(bits); })) {
        close();
        return;
    }
    arm_timer();
}

void HttpConn::close() {
    if (closed_) {
        return;
    }
    closed_ = true;
    state_ = State::Closed;
    cancel_timer();
    if (loop_ != nullptr && fd_ != INVALID_SOCKET) {
        loop_->remove(fd_);
    }
    if (stream_) {
        stream_->close();
    }
    if (on_closed_) {
        on_closed_(id_);
    }
}

void HttpConn::on_events(int bits) {
    (void)bits;
    if (closed_) {
        return;
    }
    if (state_ == State::Writing) {
        on_writable();
        return;
    }
    if (state_ == State::Reading) {
        on_readable();
    }
}

void HttpConn::update_interest(int fallback) {
    if (closed_ || fd_ == INVALID_SOCKET) {
        return;
    }
    const int want = stream_ ? stream_->wanted_events() : 0;
    loop_->set_interest(fd_, want != 0 ? want : fallback);
}

void HttpConn::on_readable() {
    if (state_ != State::Reading) {
        return;
    }

    char buffer[READ_CHUNK_SIZE];
    for (int call = 0; call < READ_CALLS_PER_EVENT; ++call) {
        const ReadResult result = stream_->read(buffer, sizeof(buffer));
        if (result.status == IoStatus::Ok) {
            if (!input_.append(buffer, result.bytes)) {
                fail(413, "request_too_large", "http request exceeds the configured body limit");
                return;
            }
            if (process_input()) {
                return;
            }
            update_interest(IO_READ);
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            update_interest(IO_READ);
            return;
        }
        close();
        return;
    }
    update_interest(IO_READ);
}

void HttpConn::on_writable() {
    if (state_ != State::Writing) {
        return;
    }

    while (output_offset_ < output_.size()) {
        const WriteResult result =
            stream_->write(output_.data() + output_offset_, output_.size() - output_offset_);
        if (result.status == IoStatus::Ok) {
            output_offset_ += result.bytes;
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            update_interest(IO_WRITE);
            return;
        }
        close();
        return;
    }

    output_.clear();
    output_offset_ = 0U;
    if (upgrade_after_write_) {
        finish_upgrade();
        return;
    }
    if (close_after_write_) {
        close();
        return;
    }

    state_ = State::Reading;
    head_parsed_ = false;
    keep_alive_ = true;
    close_after_write_ = false;
    header_end_ = 0U;
    body_length_ = 0U;
    framing_ = HttpRequestBodyFraming();
    head_ = HttpRequest();
    update_interest(IO_READ);
    arm_timer();
    if (!input_.empty()) {
        (void)process_input();
    }
}

bool HttpConn::process_input() {
    if (!head_parsed_) {
        const char* data = input_.data();
        const std::size_t size = input_.size();
        std::size_t header_end = std::string::npos;
        for (std::size_t i = 0; i + 3U < size; ++i) {
            if (data[i] == '\r' && data[i + 1U] == '\n' && data[i + 2U] == '\r'
                && data[i + 3U] == '\n') {
                header_end = i;
                break;
            }
        }
        if (header_end == std::string::npos) {
            if (size > max_header_bytes_) {
                fail(431, "request_header_too_large", "http request headers are too large");
                return true;
            }
            return false;
        }
        if (header_end + 4U > max_header_bytes_) {
            fail(431, "request_header_too_large", "http request headers are too large");
            return true;
        }

        HttpRequest head;
        try {
            head_ = parse_http_request_head(std::string(data, header_end));
            framing_ = request_body_framing_from_headers(head_.headers);
        } catch (const HttpRequestError& ex) {
            fail(400, "bad_request", ex.what());
            return true;
        } catch (const HttpProtocolError& ex) {
            fail(400, "bad_request", ex.what());
            return true;
        }

        keep_alive_ = !connection_close_requested(head_);
        if (first_request_) {
            first_request_ = false;
            if (on_first_request_) {
                on_first_request_();
            }
        }
        header_end_ = header_end + 4U;
        head_parsed_ = true;

        if (framing_.chunked) {
            scanner_.reset(header_end_, max_body_bytes_);
        } else {
            body_length_ = framing_.content_length;
            if (body_length_ > max_body_bytes_) {
                fail(413, "request_too_large", "http request body exceeds the configured limit");
                return true;
            }
        }
    }

    std::size_t total = 0U;
    if (framing_.chunked) {
        if (!scanner_.feed(input_.data(), input_.size())) {
            return false;
        }
        if (scanner_.failed()) {
            fail(400, "bad_request", "invalid chunked http request body");
            return true;
        }
        total = scanner_.end_offset;
    } else {
        total = header_end_ + body_length_;
        if (input_.size() < total) {
            return false;
        }
    }

    if (shutting_down_ != nullptr && shutting_down_->load()) {
        fail(503, "shutting_down", "daemon is shutting down");
        return true;
    }

    HttpRequest request = head_;
    head_ = HttpRequest();
    if (framing_.chunked) {
        try {
            request.body = decode_http_chunked_body(
                std::string(input_.data() + header_end_, total - header_end_)
            );
        } catch (const HttpProtocolError& ex) {
            fail(400, "bad_request", ex.what());
            return true;
        }
    } else {
        request.body.assign(input_.data() + header_end_, body_length_);
    }

    input_.consume(total);
    head_parsed_ = false;
    header_end_ = 0U;
    body_length_ = 0U;
    framing_ = HttpRequestBodyFraming();
    dispatch(request);
    return true;
}

void HttpConn::dispatch(HttpRequest& request) {
    const server_contract::RouteId route = server_contract::route_id_for_path(request.path);

    if (route == server_contract::ROUTE_UNKNOWN) {
        begin_write(make_rpc_error_response(404, "not_found", "unknown endpoint"), keep_alive_);
        return;
    }

    if (ReactorRouter::runs_inline(route)) {
        begin_write(router_->dispatch(route, request), keep_alive_);
        return;
    }

    if (route == server_contract::ROUTE_PORT_TUNNEL) {
        begin_tunnel_upgrade(request);
        return;
    }

    if (route == server_contract::ROUTE_EXEC_START || route == server_contract::ROUTE_EXEC_WRITE) {
        if (exec_ == nullptr) {
            begin_write(
                make_rpc_error_response(500, "internal_error", "exec service unavailable"),
                false
            );
            return;
        }
        state_ = State::WaitingHandler;
        cancel_timer();
        arm_timer_with(EXEC_REQUEST_SAFETY_MS);
        (void)loop_->set_interest(fd_, 0);

        std::shared_ptr<HttpRequest> shared_request(new HttpRequest());
        *shared_request = std::move(request);
        std::shared_ptr<HttpConn> self = shared_from_this();
        EventLoop* owner = loop_;
        std::function<void(HttpResponse)> done = [self, owner](HttpResponse response) {
            std::shared_ptr<HttpResponse> held(new HttpResponse(response));
            owner->post([self, held]() { self->finish_handler(*held); });
        };
        if (route == server_contract::ROUTE_EXEC_START) {
            exec_->start_async(shared_request, done);
        } else {
            exec_->write_async(shared_request, done);
        }
        return;
    }

    if (!ReactorRouter::is_migrated(route)) {
        begin_write(
            make_rpc_error_response(
                501,
                "not_migrated",
                "route is not available on the reactor server yet"
            ),
            keep_alive_
        );
        return;
    }

    state_ = State::WaitingHandler;
    cancel_timer();
    (void)loop_->set_interest(fd_, 0);
    std::shared_ptr<HttpConn> self = shared_from_this();
    std::shared_ptr<HttpRequest> shared_request(new HttpRequest());
    *shared_request = std::move(request);

    const bool accepted = pool_->post([self, shared_request, route]() {
        std::shared_ptr<HttpResponse> response(
            new HttpResponse(self->router_->dispatch(route, *shared_request))
        );
        self->loop_->post([self, response]() { self->on_handler_done(response); });
    });
    if (!accepted) {
        begin_write(make_rpc_error_response(503, "busy", "blocking worker pool is full"), false);
    }
}

void HttpConn::on_handler_done(const std::shared_ptr<HttpResponse>& response) {
    if (closed_ || state_ != State::WaitingHandler) {
        return;
    }
    begin_write(*response, keep_alive_);
}

void HttpConn::begin_tunnel_upgrade(const HttpRequest& request) {
    if (tunnel_ == nullptr) {
        begin_write(
            make_rpc_error_response(500, "internal_error", "port tunnel unavailable"),
            false
        );
        return;
    }
    HttpResponse error;
    if (!tunnel_->validate_upgrade(request, &error)) {
        begin_write(error, false);
        return;
    }

    std::map<std::string, std::string> headers;
    headers[request_id_header_name()] = request_id_for_request(request);
    output_ = render_http_upgrade_response(server_contract::PORT_TUNNEL_UPGRADE_TOKEN, headers);
    output_offset_ = 0U;
    state_ = State::Writing;
    upgrade_after_write_ = true;
    update_interest(IO_WRITE);
    arm_timer_with(EXEC_REQUEST_SAFETY_MS);
    on_writable();
}

void HttpConn::finish_upgrade() {
    if (closed_) {
        return;
    }
    std::shared_ptr<Stream> transport = stream_;
    stream_.reset();
    std::string initial_input = input_.take();

    closed_ = true;
    state_ = State::Closed;
    cancel_timer();
    loop_->remove(fd_);
    if (on_closed_) {
        on_closed_(id_);
    }
    if (tunnel_ != nullptr) {
        tunnel_->adopt(transport, initial_input);
    }
}

void HttpConn::begin_write(const HttpResponse& response, bool keep_alive) {
    if (closed_) {
        return;
    }
    if (state_ == State::Writing) {
        close();
        return;
    }
    cancel_timer();
    keep_alive_ = keep_alive;
    close_after_write_ = !keep_alive;
    output_ = render_http_response(response);
    output_offset_ = 0U;
    state_ = State::Writing;
    update_interest(IO_WRITE);
    arm_timer();
    on_writable();
}

void HttpConn::fail(int status, const char* code, const std::string& message) {
    if (closed_) {
        return;
    }
    keep_alive_ = false;
    begin_write(make_rpc_error_response(status, code, message), false);
}

void HttpConn::arm_timer() {
    arm_timer_with(idle_timeout_ms_);
}

void HttpConn::arm_timer_with(unsigned long timeout_ms) {
    cancel_timer();
    if (timeout_ms == 0UL) {
        return;
    }
    std::weak_ptr<HttpConn> weak = shared_from_this();
    timer_ = loop_->add_timer(
        platform::monotonic_ms() + static_cast<std::uint64_t>(timeout_ms),
        [weak]() {
            std::shared_ptr<HttpConn> self = weak.lock();
            if (self) {
                self->timer_armed_ = false;
                self->close();
            }
        }
    );
    timer_armed_ = true;
}

void HttpConn::cancel_timer() {
    if (!timer_armed_) {
        return;
    }
    loop_->cancel_timer(timer_);
    timer_armed_ = false;
}
