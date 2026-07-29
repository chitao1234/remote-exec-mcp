#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#endif

#include "http/http_codec.h"
#include "http/http_request.h"
#include "http/server_transport.h"
#include "platform/deadline.h"
#include "platform/socket.h"

namespace {

const std::size_t HTTP_READ_BUFFER_SIZE = 4U * 1024U;
const std::size_t HTTP_RAW_BUFFER_COMPACT_THRESHOLD = 8U * 1024U;
const std::size_t HTTP_MAX_CHUNK_LINE_SIZE = 4U * 1024U;
const unsigned long DEFAULT_CONTROLLED_READ_POLL_TIMEOUT_MS = 1000UL;

std::size_t parse_chunk_size_line(const std::string& line) {
    try {
        return parse_http_chunk_size_line(line);
    } catch (const HttpProtocolError& ex) {
        throw BadHttpRequest(ex.what());
    }
}

bool has_idle_deadline(const HttpReadControl& read_control) {
    return read_control.idle_timeout_ms != 0UL;
}

std::uint64_t read_deadline_after(const HttpReadControl* read_control) {
    if (read_control == nullptr || !has_idle_deadline(*read_control)) {
        return 0U;
    }
    return platform::monotonic_deadline_after_ms(read_control->idle_timeout_ms);
}

void reset_read_deadline(const HttpReadControl* read_control, std::uint64_t* deadline_ms) {
    if (read_control != nullptr && has_idle_deadline(*read_control)) {
        *deadline_ms = platform::monotonic_deadline_after_ms(read_control->idle_timeout_ms);
    }
}

bool read_deadline_expired(const HttpReadControl& read_control, std::uint64_t deadline_ms) {
    return has_idle_deadline(read_control) && platform::monotonic_deadline_expired(deadline_ms);
}

bool deadline_expired(std::uint64_t deadline_ms) {
    return deadline_ms != 0U && platform::monotonic_deadline_expired(deadline_ms);
}

unsigned long controlled_read_wait_ms(
    const HttpReadControl& read_control,
    std::uint64_t deadline_ms
) {
    unsigned long wait_ms = read_control.poll_timeout_ms;
    if (wait_ms == 0UL) {
        wait_ms = has_idle_deadline(read_control) ? read_control.idle_timeout_ms
                                                  : DEFAULT_CONTROLLED_READ_POLL_TIMEOUT_MS;
    }
    if (has_idle_deadline(read_control)) {
        const unsigned long remaining_ms = platform::monotonic_deadline_remaining_ms(deadline_ms);
        wait_ms = remaining_ms < wait_ms ? remaining_ms : wait_ms;
    }
    return wait_ms == 0UL ? 1UL : wait_ms;
}

bool wait_for_controlled_read(
    ConnectionTransport& client,
    const HttpReadControl& read_control,
    std::uint64_t deadline_ms
) {
    for (;;) {
        if (read_control.should_stop()) {
            return false;
        }
        if (read_deadline_expired(read_control, deadline_ms)) {
            return true;
        }

        const int ready = client.wait_readable(controlled_read_wait_ms(read_control, deadline_ms));
        if (ready > 0) {
            return true;
        }
        if (ready == 0) {
            continue;
        }
        if (read_control.should_stop()) {
            return false;
        }
        throw std::runtime_error(socket_error_message("select"));
    }
}

} // namespace

bool try_read_http_request_head(
    ConnectionTransport& client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
) {
    std::string data;
    char buffer[HTTP_READ_BUFFER_SIZE];
    std::size_t search_offset = 0;

    for (;;) {
        const int received = client.read(buffer, sizeof(buffer));
        if (received == 0) {
            if (data.empty()) {
                return false;
            }
            break;
        }
        if (received < 0) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                if (data.empty()) {
                    return false;
                }
                throw BadHttpRequest("incomplete http request");
            }
            throw std::runtime_error(socket_error_message("recv"));
        }

        data.append(buffer, received);
        const std::size_t header_end = data.find("\r\n\r\n", search_offset);
        if (header_end == std::string::npos) {
            if (data.size() > max_header_bytes) {
                throw BadHttpRequest("http request headers too large");
            }
            search_offset = data.size() > 3U ? data.size() - 3U : 0U;
            continue;
        }

        if (header_end + 4U > max_header_bytes) {
            throw BadHttpRequest("http request headers too large");
        }

        head->raw_headers = data.substr(0, header_end);
        head->initial_body = data.substr(header_end + 4U);
        return true;
    }

    throw BadHttpRequest("incomplete http request");
}

bool try_read_http_request_head_controlled(
    ConnectionTransport& client,
    std::size_t max_header_bytes,
    const HttpReadControl& read_control,
    HttpRequestHead* head
) {
    std::string data;
    char buffer[HTTP_READ_BUFFER_SIZE];
    std::size_t search_offset = 0;
    std::uint64_t deadline_ms = read_deadline_after(&read_control);

    for (;;) {
        if (!wait_for_controlled_read(client, read_control, deadline_ms)) {
            return false;
        }
        if (read_deadline_expired(read_control, deadline_ms)) {
            if (data.empty()) {
                return false;
            }
            throw BadHttpRequest("incomplete http request");
        }

        const int received = client.read(buffer, sizeof(buffer));
        if (received == 0) {
            if (data.empty()) {
                return false;
            }
            break;
        }
        if (received < 0) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                continue;
            }
            if (read_control.should_stop()) {
                return false;
            }
            throw std::runtime_error(socket_error_message("recv"));
        }

        reset_read_deadline(&read_control, &deadline_ms);
        data.append(buffer, received);
        const std::size_t header_end = data.find("\r\n\r\n", search_offset);
        if (header_end == std::string::npos) {
            if (data.size() > max_header_bytes) {
                throw BadHttpRequest("http request headers too large");
            }
            search_offset = data.size() > 3U ? data.size() - 3U : 0U;
            continue;
        }

        if (header_end + 4U > max_header_bytes) {
            throw BadHttpRequest("http request headers too large");
        }

        head->raw_headers = data.substr(0, header_end);
        head->initial_body = data.substr(header_end + 4U);
        return true;
    }

    throw BadHttpRequest("incomplete http request");
}

HttpRequestHead read_http_request_head(ConnectionTransport& client, std::size_t max_header_bytes) {
    HttpRequestHead head;
    if (try_read_http_request_head(client, max_header_bytes, &head)) {
        return head;
    }

    throw BadHttpRequest("incomplete http request");
}

bool try_read_http_request_head(
    SOCKET client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    return try_read_http_request_head(*transport, max_header_bytes, head);
}

bool try_read_http_request_head_controlled(
    SOCKET client,
    std::size_t max_header_bytes,
    const HttpReadControl& read_control,
    HttpRequestHead* head
) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    return try_read_http_request_head_controlled(*transport, max_header_bytes, read_control, head);
}

HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    return read_http_request_head(*transport, max_header_bytes);
}

HttpRequestBodyStream::HttpRequestBodyStream(
    const std::shared_ptr<ConnectionTransport>& client,
    const std::string& initial_body,
    const HttpRequestBodyFraming& framing,
    std::size_t max_body_bytes
)
    : HttpRequestBodyStream(client, initial_body, framing, max_body_bytes, nullptr) {
}

HttpRequestBodyStream::HttpRequestBodyStream(
    const std::shared_ptr<ConnectionTransport>& client,
    const std::string& initial_body,
    const HttpRequestBodyFraming& framing,
    std::size_t max_body_bytes,
    const HttpReadControl* read_control
)
    : client_(client), read_control_(read_control),
      read_deadline_ms_(read_deadline_after(read_control)), raw_(initial_body), raw_offset_(0),
      framing_(framing), decoded_size_(0), max_body_bytes_(max_body_bytes),
      remaining_content_length_(framing.content_length), remaining_chunk_size_(0),
      chunked_finished_(false) {
    if (!framing_.chunked && remaining_content_length_ > max_body_bytes_) {
        throw BadHttpRequest("http request body too large");
    }
}

HttpRequestBodyStream::HttpRequestBodyStream(
    SOCKET client,
    const std::string& initial_body,
    const HttpRequestBodyFraming& framing,
    std::size_t max_body_bytes
)
    : HttpRequestBodyStream(
          make_borrowed_connection_transport(client),
          initial_body,
          framing,
          max_body_bytes,
          nullptr
      ) {
}

HttpRequestBodyStream::HttpRequestBodyStream(
    SOCKET client,
    const std::string& initial_body,
    const HttpRequestBodyFraming& framing,
    std::size_t max_body_bytes,
    const HttpReadControl* read_control
)
    : HttpRequestBodyStream(
          make_borrowed_connection_transport(client),
          initial_body,
          framing,
          max_body_bytes,
          read_control
      ) {
}

std::size_t HttpRequestBodyStream::read(char* data, std::size_t max_size) {
    if (max_size == 0U) {
        return 0;
    }
    if (framing_.chunked) {
        return read_chunked_body(data, max_size);
    }
    return read_content_length_body(data, max_size);
}

bool HttpRequestBodyStream::fully_consumed() const {
    return !has_remaining_body();
}

bool HttpRequestBodyStream::has_remaining_body() const {
    if (framing_.chunked) {
        return !chunked_finished_ || remaining_chunk_size_ != 0U || raw_.size() > raw_offset_;
    }
    return remaining_content_length_ != 0U;
}

bool HttpRequestBodyStream::discard_remaining_bounded(
    unsigned long timeout_ms,
    std::size_t max_bytes
) {
    if (!has_remaining_body()) {
        return true;
    }

    const std::uint64_t deadline_ms =
        timeout_ms == 0UL ? 0U : platform::monotonic_deadline_after_ms(timeout_ms);
    if (framing_.chunked) {
        return try_discard_chunked_body(max_bytes, deadline_ms);
    }
    return try_discard_content_length_body(max_bytes, deadline_ms);
}

std::size_t HttpRequestBodyStream::read_content_length_body(char* data, std::size_t max_size) {
    if (remaining_content_length_ == 0U) {
        return 0;
    }

    const std::size_t requested =
        remaining_content_length_ < max_size ? remaining_content_length_ : max_size;
    ensure_raw_available(1);
    const std::size_t available = raw_.size() - raw_offset_;
    const std::size_t copied = requested < available ? requested : available;
    std::copy(raw_.data() + raw_offset_, raw_.data() + raw_offset_ + copied, data);
    consume_raw(copied);
    remaining_content_length_ -= copied;
    decoded_size_ += copied;
    return copied;
}

std::size_t HttpRequestBodyStream::read_chunked_body(char* data, std::size_t max_size) {
    if (chunked_finished_) {
        return 0;
    }

    while (remaining_chunk_size_ == 0U) {
        ensure_raw_line();
        const std::size_t line_end = raw_.find("\r\n", raw_offset_);
        const std::size_t chunk_size =
            parse_chunk_size_line(raw_.substr(raw_offset_, line_end - raw_offset_));
        consume_raw(line_end + 2U - raw_offset_);

        if (chunk_size == 0U) {
            consume_chunk_trailers();
            chunked_finished_ = true;
            return 0;
        }
        if (chunk_size > max_body_bytes_ - decoded_size_) {
            throw BadHttpRequest("http request body too large");
        }
        remaining_chunk_size_ = chunk_size;
    }

    ensure_raw_available(1);
    const std::size_t available = raw_.size() - raw_offset_;
    std::size_t copied = remaining_chunk_size_ < max_size ? remaining_chunk_size_ : max_size;
    copied = copied < available ? copied : available;
    std::copy(raw_.data() + raw_offset_, raw_.data() + raw_offset_ + copied, data);
    consume_raw(copied);
    remaining_chunk_size_ -= copied;
    decoded_size_ += copied;

    if (remaining_chunk_size_ == 0U) {
        ensure_raw_available(2);
        if (raw_.compare(raw_offset_, 2U, "\r\n") != 0) {
            throw BadHttpRequest("invalid chunked request body");
        }
        consume_raw(2);
    }

    return copied;
}

void HttpRequestBodyStream::ensure_raw_available(std::size_t size) {
    while (raw_.size() - raw_offset_ < size) {
        append_from_socket();
    }
}

void HttpRequestBodyStream::ensure_raw_line() {
    while (raw_.find("\r\n", raw_offset_) == std::string::npos) {
        if (raw_.size() - raw_offset_ > HTTP_MAX_CHUNK_LINE_SIZE) {
            throw BadHttpRequest("chunked request line too long");
        }
        append_from_socket();
    }
}

void HttpRequestBodyStream::append_from_socket() {
    if (read_control_ != nullptr) {
        if (!wait_for_controlled_read(*client_, *read_control_, read_deadline_ms_)) {
            throw HttpConnectionShutdown("server shutting down");
        }
        if (read_deadline_expired(*read_control_, read_deadline_ms_)) {
            throw BadHttpRequest("incomplete http request body");
        }
    }

    char buffer[HTTP_READ_BUFFER_SIZE];
    const int received = client_->read(buffer, sizeof(buffer));
    if (received == 0) {
        if (read_control_ != nullptr && read_control_->should_stop()) {
            throw HttpConnectionShutdown("server shutting down");
        }
        throw BadHttpRequest("incomplete http request body");
    }
    if (received < 0) {
        const int error = last_socket_error();
        if (read_control_ != nullptr && receive_timeout_error(error)) {
            return;
        }
        if (receive_timeout_error(error)) {
            throw BadHttpRequest("incomplete http request body");
        }
        if (read_control_ != nullptr && read_control_->should_stop()) {
            throw HttpConnectionShutdown("server shutting down");
        }
        throw std::runtime_error(socket_error_message("recv"));
    }

    reset_read_deadline(read_control_, &read_deadline_ms_);
    raw_.append(buffer, received);
}

void HttpRequestBodyStream::consume_raw(std::size_t size) {
    raw_offset_ += size;
    if (raw_offset_ > HTTP_RAW_BUFFER_COMPACT_THRESHOLD && raw_offset_ * 2U > raw_.size()) {
        raw_.erase(0, raw_offset_);
        raw_offset_ = 0;
    }
}

void HttpRequestBodyStream::consume_chunk_trailers() {
    for (;;) {
        ensure_raw_line();
        const std::size_t line_end = raw_.find("\r\n", raw_offset_);
        if (line_end == raw_offset_) {
            consume_raw(2);
            return;
        }
        consume_raw(line_end + 2U - raw_offset_);
    }
}

bool HttpRequestBodyStream::try_discard_content_length_body(
    std::size_t max_bytes,
    std::uint64_t deadline_ms
) {
    std::size_t discarded = 0U;
    while (remaining_content_length_ != 0U) {
        if (discarded >= max_bytes || deadline_expired(deadline_ms)) {
            return false;
        }
        if (!try_ensure_raw_available_until(1U, deadline_ms)) {
            return false;
        }

        const std::size_t available = raw_.size() - raw_offset_;
        std::size_t step =
            remaining_content_length_ < available ? remaining_content_length_ : available;
        const std::size_t remaining_budget = max_bytes - discarded;
        if (step > remaining_budget) {
            step = remaining_budget;
        }
        if (step == 0U) {
            return false;
        }

        consume_raw(step);
        remaining_content_length_ -= step;
        decoded_size_ += step;
        discarded += step;
    }
    return true;
}

bool HttpRequestBodyStream::try_discard_chunked_body(
    std::size_t max_bytes,
    std::uint64_t deadline_ms
) {
    std::size_t discarded = 0U;
    while (!chunked_finished_) {
        while (remaining_chunk_size_ == 0U) {
            if (!try_ensure_raw_line_until(deadline_ms)) {
                return false;
            }
            const std::size_t line_end = raw_.find("\r\n", raw_offset_);
            const std::size_t chunk_size =
                parse_chunk_size_line(raw_.substr(raw_offset_, line_end - raw_offset_));
            consume_raw(line_end + 2U - raw_offset_);

            if (chunk_size == 0U) {
                if (!try_consume_chunk_trailers_until(deadline_ms)) {
                    return false;
                }
                chunked_finished_ = true;
                return true;
            }
            remaining_chunk_size_ = chunk_size;
        }

        while (remaining_chunk_size_ != 0U) {
            if (discarded >= max_bytes || deadline_expired(deadline_ms)) {
                return false;
            }
            if (!try_ensure_raw_available_until(1U, deadline_ms)) {
                return false;
            }

            const std::size_t available = raw_.size() - raw_offset_;
            std::size_t step =
                remaining_chunk_size_ < available ? remaining_chunk_size_ : available;
            const std::size_t remaining_budget = max_bytes - discarded;
            if (step > remaining_budget) {
                step = remaining_budget;
            }
            if (step == 0U) {
                return false;
            }

            consume_raw(step);
            remaining_chunk_size_ -= step;
            decoded_size_ += step;
            discarded += step;
        }

        if (!try_ensure_raw_available_until(2U, deadline_ms)) {
            return false;
        }
        if (raw_.compare(raw_offset_, 2U, "\r\n") != 0) {
            throw BadHttpRequest("invalid chunked request body");
        }
        consume_raw(2U);
    }

    return true;
}

bool HttpRequestBodyStream::try_append_from_socket_until(std::uint64_t deadline_ms) {
    if (deadline_expired(deadline_ms)) {
        return false;
    }

    const unsigned long wait_ms = deadline_ms == 0U
                                      ? DEFAULT_CONTROLLED_READ_POLL_TIMEOUT_MS
                                      : platform::monotonic_deadline_remaining_ms(deadline_ms);
    const int ready = client_->wait_readable(wait_ms == 0UL ? 1UL : wait_ms);
    if (ready <= 0) {
        return false;
    }

    char buffer[HTTP_READ_BUFFER_SIZE];
    const int received = client_->read(buffer, sizeof(buffer));
    if (received <= 0) {
        return false;
    }

    raw_.append(buffer, received);
    return true;
}

bool HttpRequestBodyStream::try_ensure_raw_available_until(
    std::size_t size,
    std::uint64_t deadline_ms
) {
    while (raw_.size() - raw_offset_ < size) {
        if (!try_append_from_socket_until(deadline_ms)) {
            return false;
        }
    }
    return true;
}

bool HttpRequestBodyStream::try_ensure_raw_line_until(std::uint64_t deadline_ms) {
    while (raw_.find("\r\n", raw_offset_) == std::string::npos) {
        if (raw_.size() - raw_offset_ > HTTP_MAX_CHUNK_LINE_SIZE) {
            throw BadHttpRequest("chunked request line too long");
        }
        if (!try_append_from_socket_until(deadline_ms)) {
            return false;
        }
    }
    return true;
}

bool HttpRequestBodyStream::try_consume_chunk_trailers_until(std::uint64_t deadline_ms) {
    for (;;) {
        if (!try_ensure_raw_line_until(deadline_ms)) {
            return false;
        }
        const std::size_t line_end = raw_.find("\r\n", raw_offset_);
        if (line_end == raw_offset_) {
            consume_raw(2U);
            return true;
        }
        consume_raw(line_end + 2U - raw_offset_);
    }
}

HttpChunkedResponseWriter::HttpChunkedResponseWriter(
    const std::shared_ptr<ConnectionTransport>& client
)
    : client_(client) {
}

HttpChunkedResponseWriter::HttpChunkedResponseWriter(SOCKET client)
    : client_(make_borrowed_connection_transport(client)) {
}

void HttpChunkedResponseWriter::write_chunk(const char* data, std::size_t size) {
    std::ostringstream header;
    header << std::hex << size << "\r\n";
    send_all(*client_, header.str());
    if (size != 0U) {
        send_all_bytes(*client_, data, size);
    }
    send_all(*client_, "\r\n");
}

void HttpChunkedResponseWriter::write_chunk(const std::string& data) {
    write_chunk(data.data(), data.size());
}

void HttpChunkedResponseWriter::finish() {
    send_all(*client_, "0\r\n\r\n");
}

std::string read_request_body_to_string(HttpRequestBodyStream* body) {
    std::string output;
    char buffer[8192];
    for (;;) {
        const std::size_t received = body->read(buffer, sizeof(buffer));
        if (received == 0U) {
            return output;
        }
        output.append(buffer, received);
    }
}

void send_all(ConnectionTransport& client, const std::string& data) {
    send_all_bytes(client, data.data(), data.size());
}

void send_all_bytes(ConnectionTransport& client, const char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int sent = client.write(data + offset, size - offset);
        if (sent <= 0) {
            const int error = last_socket_error();
            throw SocketSendError(
                socket_error_message("send"),
                peer_disconnected_send_error(error)
            );
        }
        offset += static_cast<std::size_t>(sent);
    }
}

void send_http_response(ConnectionTransport& client, const HttpResponse& response) {
    send_all(client, render_http_response(response));
}

void send_http_response_head(ConnectionTransport& client, const HttpResponse& response) {
    send_all(client, render_http_response_head(response));
}

void send_http_upgrade_response(
    ConnectionTransport& client,
    const std::string& upgrade_token,
    const std::map<std::string, std::string>& headers
) {
    send_all(client, render_http_upgrade_response(upgrade_token, headers));
}

void send_all(SOCKET client, const std::string& data) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    send_all(*transport, data);
}

void send_all_bytes(SOCKET client, const char* data, std::size_t size) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    send_all_bytes(*transport, data, size);
}

void send_http_response(SOCKET client, const HttpResponse& response) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    send_http_response(*transport, response);
}

void send_http_response_head(SOCKET client, const HttpResponse& response) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    send_http_response_head(*transport, response);
}

void send_http_upgrade_response(
    SOCKET client,
    const std::string& upgrade_token,
    const std::map<std::string, std::string>& headers
) {
    std::shared_ptr<ConnectionTransport> transport = make_borrowed_connection_transport(client);
    send_http_upgrade_response(*transport, upgrade_token, headers);
}

SOCKET create_listener(const DaemonConfig& config) {
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

    SOCKET listener = INVALID_SOCKET;
    for (std::size_t i = 0; i < addresses.size(); ++i) {
        const SocketAddress& current = addresses[i];
        listener = create_socket_cloexec(current.family, current.socktype, current.protocol);
        if (listener == INVALID_SOCKET) {
            continue;
        }

        (void)set_socket_reuseaddr(listener);

        if (bind_socket(listener, current.sockaddr_ptr(), current.address_len) == 0) {
            break;
        }

        close_socket(listener);
        listener = INVALID_SOCKET;
    }

    if (listener == INVALID_SOCKET) {
        throw std::runtime_error(socket_error_message("bind"));
    }

    if (listen_socket(listener, SOMAXCONN) != 0) {
        close_socket(listener);
        throw std::runtime_error(socket_error_message("listen"));
    }

    return listener;
}
