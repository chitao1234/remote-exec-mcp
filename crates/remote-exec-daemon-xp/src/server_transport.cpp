#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>

#include <winsock2.h>
#include <windows.h>
#include <ws2tcpip.h>

#include "http_request.h"
#include "server_transport.h"

std::string read_http_request(SOCKET client) {
    std::string data;
    char buffer[4096];
    std::size_t expected_size = 0;
    bool parsed_headers = false;

    for (;;) {
        const int received = recv(client, buffer, sizeof(buffer), 0);
        if (received <= 0) {
            break;
        }

        data.append(buffer, received);

        if (!parsed_headers) {
            const std::size_t header_end = data.find("\r\n\r\n");
            if (header_end != std::string::npos) {
                parsed_headers = true;
                const HttpRequest request = parse_http_request(data);
                expected_size = header_end + 4;
                const std::string content_length = request.header("content-length");
                if (!content_length.empty()) {
                    expected_size += static_cast<std::size_t>(std::atoi(content_length.c_str()));
                }
            }
        }

        if (parsed_headers && data.size() >= expected_size) {
            break;
        }
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
            throw std::runtime_error("send failed");
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

    SOCKET listener = socket(result->ai_family, result->ai_socktype, result->ai_protocol);
    if (listener == INVALID_SOCKET) {
        freeaddrinfo(result);
        throw std::runtime_error("socket failed");
    }

    const char yes = 1;
    setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));

    if (bind(listener, result->ai_addr, static_cast<int>(result->ai_addrlen)) != 0) {
        freeaddrinfo(result);
        closesocket(listener);
        throw std::runtime_error("bind failed");
    }
    freeaddrinfo(result);

    if (listen(listener, SOMAXCONN) != 0) {
        closesocket(listener);
        throw std::runtime_error("listen failed");
    }

    return listener;
}
