#pragma once

#include <cstddef>
#include <memory>

#include "platform/socket.h"

enum class IoStatus {
    Ok,
    Eof,
    WouldBlock,
    Error,
};

struct ReadResult {
    ReadResult(IoStatus status_value, std::size_t bytes_value)
        : status(status_value), bytes(bytes_value) {}

    IoStatus status;
    std::size_t bytes;
};

struct WriteResult {
    WriteResult(IoStatus status_value, std::size_t bytes_value)
        : status(status_value), bytes(bytes_value) {}

    IoStatus status;
    std::size_t bytes;
};

// Non-blocking byte stream. Implementations never apply their own timeouts;
// deadlines belong to the event loop that owns the descriptor.
class Stream {
public:
    virtual ~Stream() {}
    virtual SOCKET native_fd() const = 0;
    virtual ReadResult read(char* data, std::size_t size) = 0;
    virtual WriteResult write(const char* data, std::size_t size) = 0;
    virtual void close() = 0;
};

std::shared_ptr<Stream> make_socket_stream(UniqueSocket socket);
