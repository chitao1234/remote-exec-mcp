#pragma once

#include <cstddef>
#include <memory>

#include "platform/socket.h"

class ConnectionTransport {
public:
    virtual ~ConnectionTransport() {}

    virtual SOCKET native_socket() const = 0;
    virtual int wait_readable(unsigned long timeout_ms) = 0;
    virtual int read(char* data, std::size_t size) = 0;
    virtual int write(const char* data, std::size_t size) = 0;
    virtual void set_timeout_ms(unsigned long timeout_ms) = 0;
    virtual void shutdown() = 0;
};

std::shared_ptr<ConnectionTransport> make_plain_connection_transport(UniqueSocket socket);
std::shared_ptr<ConnectionTransport> make_borrowed_connection_transport(SOCKET socket);
