#include <algorithm>
#include <cerrno>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#else
#include <netdb.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "http_codec.h"
#include "http_request.h"
#include "server_transport.h"

void close_socket(SOCKET socket) {
#ifdef _WIN32
    closesocket(socket);
#else
    close(socket);
#endif
}

void shutdown_socket(SOCKET socket) {
#ifdef _WIN32
    shutdown(socket, SD_BOTH);
#else
    shutdown(socket, SHUT_RDWR);
#endif
}

int last_socket_error() {
#ifdef _WIN32
    return WSAGetLastError();
#else
    return errno;
#endif
}

std::string socket_error_message(const std::string& operation) {
    std::ostringstream out;
    out << operation << " failed";
#ifndef _WIN32
    out << ": " << std::strerror(errno);
#else
    out << ": " << WSAGetLastError();
#endif
    return out.str();
}

bool would_block_error(int error) {
#ifdef _WIN32
    return error == WSAEWOULDBLOCK;
#else
    return error == EAGAIN || error == EWOULDBLOCK;
#endif
}

bool peer_disconnected_send_error(int error) {
#ifdef _WIN32
    return error == WSAECONNABORTED || error == WSAECONNRESET || error == WSAESHUTDOWN;
#else
    return error == EPIPE || error == ECONNRESET || error == ENOTCONN;
#endif
}

namespace {

std::size_t parse_chunk_size_line(const std::string& line) {
    try {
        return parse_http_chunk_size_line(line);
    } catch (const HttpProtocolError& ex) {
        throw BadHttpRequest(ex.what());
    }
}

}  // namespace

UniqueSocket::UniqueSocket() : socket_(INVALID_SOCKET) {}

UniqueSocket::UniqueSocket(SOCKET socket) : socket_(socket) {}

UniqueSocket::~UniqueSocket() {
    reset();
}

UniqueSocket::UniqueSocket(UniqueSocket&& other) : socket_(other.release()) {}

UniqueSocket& UniqueSocket::operator=(UniqueSocket&& other) {
    if (this != &other) {
        reset(other.release());
    }
    return *this;
}

SOCKET UniqueSocket::get() const {
    return socket_;
}

bool UniqueSocket::valid() const {
    return socket_ != INVALID_SOCKET;
}

SOCKET UniqueSocket::release() {
    const SOCKET released = socket_;
    socket_ = INVALID_SOCKET;
    return released;
}

void UniqueSocket::reset(SOCKET socket) {
    if (valid()) {
        close_socket(socket_);
    }
    socket_ = socket;
}

NetworkSession::NetworkSession() {
#ifdef _WIN32
    WSADATA wsa_data;
    if (WSAStartup(MAKEWORD(2, 2), &wsa_data) != 0) {
        throw std::runtime_error("WSAStartup failed");
    }
#else
    signal(SIGPIPE, SIG_IGN);
#endif
}

NetworkSession::~NetworkSession() {
#ifdef _WIN32
    WSACleanup();
#endif
}

bool try_read_http_request_head(
    SOCKET client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
) {
    std::string data;
    char buffer[4096];

    for (;;) {
        const int received = recv(client, buffer, sizeof(buffer), 0);
        if (received == 0) {
            if (data.empty()) {
                return false;
            }
            break;
        }
        if (received < 0) {
            const int error = last_socket_error();
            if (would_block_error(error)) {
                continue;
            }
            throw std::runtime_error(socket_error_message("recv"));
        }

        data.append(buffer, received);
        const std::size_t header_end = data.find("\r\n\r\n");
        if (header_end == std::string::npos) {
            if (data.size() > max_header_bytes) {
                throw BadHttpRequest("http request headers too large");
            }
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

HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes) {
    HttpRequestHead head;
    if (try_read_http_request_head(client, max_header_bytes, &head)) {
        return head;
    }

    throw BadHttpRequest("incomplete http request");
}

HttpRequestBodyStream::HttpRequestBodyStream(
    SOCKET client,
    const std::string& initial_body,
    const HttpRequestBodyFraming& framing,
    std::size_t max_body_bytes
)
    : client_(client),
      raw_(initial_body),
      raw_offset_(0),
      framing_(framing),
      decoded_size_(0),
      max_body_bytes_(max_body_bytes),
      remaining_content_length_(framing.content_length),
      remaining_chunk_size_(0),
      chunked_finished_(false) {
    if (!framing_.chunked && remaining_content_length_ > max_body_bytes_) {
        throw BadHttpRequest("http request body too large");
    }
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
        char buffer[4096];
        const int received = recv(client_, buffer, sizeof(buffer), 0);
        if (received == 0) {
            throw BadHttpRequest("incomplete http request body");
        }
        if (received < 0) {
            const int error = last_socket_error();
            if (would_block_error(error)) {
                continue;
            }
            throw std::runtime_error(socket_error_message("recv"));
        }
        raw_.append(buffer, received);
    }
}

void HttpRequestBodyStream::ensure_raw_line() {
    while (raw_.find("\r\n", raw_offset_) == std::string::npos) {
        char buffer[4096];
        const int received = recv(client_, buffer, sizeof(buffer), 0);
        if (received == 0) {
            throw BadHttpRequest("incomplete http request body");
        }
        if (received < 0) {
            const int error = last_socket_error();
            if (would_block_error(error)) {
                continue;
            }
            throw std::runtime_error(socket_error_message("recv"));
        }
        raw_.append(buffer, received);
    }
}

void HttpRequestBodyStream::consume_raw(std::size_t size) {
    raw_offset_ += size;
    if (raw_offset_ > 8192U && raw_offset_ * 2U > raw_.size()) {
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

void send_all(SOCKET client, const std::string& data) {
    send_all_bytes(client, data.data(), data.size());
}

void send_all_bytes(SOCKET client, const char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int sent = send(
            client,
            data + offset,
            static_cast<int>(size - offset),
            0
        );
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

SOCKET create_listener(const DaemonConfig& config) {
    char port_buffer[16];
    std::snprintf(port_buffer, sizeof(port_buffer), "%d", config.listen_port);

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    hints.ai_flags = AI_PASSIVE;

    addrinfo* result = NULL;
    if (getaddrinfo(config.listen_host.c_str(), port_buffer, &hints, &result) != 0) {
        throw std::runtime_error("getaddrinfo failed");
    }

    SOCKET listener = INVALID_SOCKET;
    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        listener = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (listener == INVALID_SOCKET) {
            continue;
        }

        int yes = 1;
        setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, reinterpret_cast<const char*>(&yes), sizeof(yes));

        if (bind(listener, current->ai_addr, static_cast<int>(current->ai_addrlen)) == 0) {
            break;
        }

        close_socket(listener);
        listener = INVALID_SOCKET;
    }
    freeaddrinfo(result);

    if (listener == INVALID_SOCKET) {
        throw std::runtime_error(socket_error_message("bind"));
    }

    if (listen(listener, SOMAXCONN) != 0) {
        close_socket(listener);
        throw std::runtime_error(socket_error_message("listen"));
    }

    return listener;
}

SOCKET accept_client(SOCKET listener) {
    return accept(listener, NULL, NULL);
}
