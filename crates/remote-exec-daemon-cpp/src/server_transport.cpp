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

#include "http_request.h"
#include "server_transport.h"

namespace {

void close_socket(SOCKET socket) {
#ifdef _WIN32
    closesocket(socket);
#else
    close(socket);
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

std::size_t parse_content_length_value(const std::string& raw) {
    if (raw.empty()) {
        throw BadHttpRequest("invalid Content-Length");
    }

    std::size_t value = 0;
    for (std::size_t i = 0; i < raw.size(); ++i) {
        const char ch = raw[i];
        if (ch < '0' || ch > '9') {
            throw BadHttpRequest("invalid Content-Length");
        }
        const std::size_t digit = static_cast<std::size_t>(ch - '0');
        if (value > (std::numeric_limits<std::size_t>::max() - digit) / 10U) {
            throw BadHttpRequest("Content-Length is too large");
        }
        value = value * 10U + digit;
    }
    return value;
}

std::string trim_http_header_value(std::string value) {
    while (!value.empty() && (value[0] == ' ' || value[0] == '\t')) {
        value.erase(value.begin());
    }
    while (!value.empty() && (value[value.size() - 1] == ' ' ||
                              value[value.size() - 1] == '\t' ||
                              value[value.size() - 1] == '\r')) {
        value.erase(value.size() - 1);
    }
    return value;
}

std::size_t content_length_from_headers(const std::string& header_block) {
    std::istringstream lines(header_block);
    std::string line;
    bool found = false;
    std::size_t content_length = 0;

    while (std::getline(lines, line)) {
        if (!line.empty() && line[line.size() - 1] == '\r') {
            line.erase(line.size() - 1);
        }
        const std::size_t colon = line.find(':');
        if (colon == std::string::npos) {
            continue;
        }
        std::string name = line.substr(0, colon);
        for (std::size_t i = 0; i < name.size(); ++i) {
            name[i] = static_cast<char>(std::tolower(static_cast<unsigned char>(name[i])));
        }
        if (name != "content-length") {
            continue;
        }
        if (found) {
            throw BadHttpRequest("duplicate Content-Length");
        }
        found = true;
        content_length = parse_content_length_value(trim_http_header_value(line.substr(colon + 1)));
    }

    return found ? content_length : 0U;
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

std::string read_http_request(
    SOCKET client,
    std::size_t max_header_bytes,
    std::size_t max_body_bytes
) {
    std::string data;
    char buffer[4096];
    std::size_t expected_size = 0;
    bool parsed_headers = false;

    for (;;) {
        const int received = recv(client, buffer, sizeof(buffer), 0);
        if (received == 0) {
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

        if (!parsed_headers) {
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

            parsed_headers = true;
            const std::size_t content_length =
                content_length_from_headers(data.substr(0, header_end));
            if (content_length > max_body_bytes) {
                throw BadHttpRequest("http request body too large");
            }
            expected_size = header_end + 4U + content_length;
        }

        if (parsed_headers) {
            if (data.size() > expected_size) {
                data.resize(expected_size);
            }
            if (data.size() >= expected_size) {
                break;
            }
        }
    }

    if (!parsed_headers) {
        throw BadHttpRequest("incomplete http request");
    }

    return data;
}

void send_all(SOCKET client, const std::string& data) {
    std::size_t offset = 0;
    while (offset < data.size()) {
        const int sent = send(
            client,
            data.data() + offset,
            static_cast<int>(data.size() - offset),
            0
        );
        if (sent <= 0) {
            throw std::runtime_error(socket_error_message("send"));
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
