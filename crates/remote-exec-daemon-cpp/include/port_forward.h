#pragma once

#include <map>
#include <memory>
#include <stdexcept>
#include <set>
#include <string>
#include <cstdint>

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

enum PortResourceState {
    PORT_RESOURCE_OPEN = 0,
    PORT_RESOURCE_CLOSING = 1,
    PORT_RESOURCE_CLOSED = 2
};

struct TcpConnection {
    TcpConnection(SOCKET socket, const std::string& lease_id);

    UniqueSocket socket;
    BasicMutex state_mutex;
    BasicMutex read_mutex;
    BasicMutex write_mutex;
    PortResourceState state;
    std::string lease_id;
};

struct SharedSocket {
    SharedSocket(SOCKET socket, const std::string& lease_id);

    UniqueSocket socket;
    BasicMutex state_mutex;
    PortResourceState state;
    std::string lease_id;
};

struct LeaseEntry {
    std::uint64_t expires_at_ms;
    std::set<std::string> binds;
    std::set<std::string> connections;
};

class PortForwardStore {
public:
    PortForwardStore();
    ~PortForwardStore();

    Json listen(
        const std::string& endpoint,
        const std::string& protocol,
        const std::string& lease_id,
        std::uint64_t lease_ttl_ms
    );
    Json listen_accept(const std::string& bind_id);
    Json listen_close(const std::string& bind_id);
    Json lease_renew(const std::string& lease_id, std::uint64_t lease_ttl_ms);
    Json connect(
        const std::string& endpoint,
        const std::string& protocol,
        const std::string& lease_id,
        std::uint64_t lease_ttl_ms
    );
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
    void sweep_expired_leases();
    void register_bind_lease(
        const std::string& lease_id,
        std::uint64_t lease_ttl_ms,
        const std::string& bind_id
    );
    void register_connection_lease(
        const std::string& lease_id,
        std::uint64_t lease_ttl_ms,
        const std::string& connection_id
    );
    void renew_lease(const std::string& lease_id, std::uint64_t lease_ttl_ms);
    void track_connection_lease(const std::string& lease_id, const std::string& connection_id);
    void untrack_bind_lease(const std::string& lease_id, const std::string& bind_id);
    void untrack_connection_lease(const std::string& lease_id, const std::string& connection_id);
    std::uint64_t lease_deadline_ms(std::uint64_t lease_ttl_ms) const;

    BasicMutex mutex_;
    std::map<std::string, std::shared_ptr<SharedSocket> > tcp_listeners_;
    std::map<std::string, std::shared_ptr<SharedSocket> > udp_sockets_;
    std::map<std::string, std::shared_ptr<TcpConnection> > tcp_connections_;
    std::map<std::string, LeaseEntry> leases_;
};
