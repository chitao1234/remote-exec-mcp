#pragma once

#include <map>
#include <memory>
#include <stdexcept>
#include <string>

#include "basic_mutex.h"
#include "http_helpers.h"
#include "server_transport.h"

class PortForwardError : public std::runtime_error {
public:
    PortForwardError(int status, const std::string& code, const std::string& message);

    int status() const;
    const std::string& code() const;

private:
    int status_;
    std::string code_;
};

struct TcpConnection {
    explicit TcpConnection(SOCKET socket);

    UniqueSocket socket;
    BasicMutex state_mutex;
    BasicMutex read_mutex;
    BasicMutex write_mutex;
    bool closed;
};

struct SharedSocket {
    explicit SharedSocket(SOCKET socket);

    UniqueSocket socket;
    BasicMutex state_mutex;
    bool closed;
};

class PortForwardStore {
public:
    PortForwardStore();
    ~PortForwardStore();

    Json listen(const std::string& endpoint, const std::string& protocol);
    Json listen_accept(const std::string& bind_id);
    Json listen_close(const std::string& bind_id);
    Json connect(const std::string& endpoint, const std::string& protocol);
    Json connection_read(const std::string& connection_id);
    Json connection_write(const std::string& connection_id, const std::string& data);
    Json connection_close(const std::string& connection_id);
    Json udp_datagram_read(const std::string& bind_id);
    Json udp_datagram_write(
        const std::string& bind_id,
        const std::string& peer,
        const std::string& data
    );

private:
    std::shared_ptr<TcpConnection> tcp_connection(const std::string& connection_id);
    std::shared_ptr<SharedSocket> tcp_listener(const std::string& bind_id);
    std::shared_ptr<SharedSocket> udp_socket(const std::string& bind_id);

    BasicMutex mutex_;
    std::map<std::string, std::shared_ptr<SharedSocket> > tcp_listeners_;
    std::map<std::string, std::shared_ptr<SharedSocket> > udp_sockets_;
    std::map<std::string, std::shared_ptr<TcpConnection> > tcp_connections_;
};
