#include "core/stream.h"

#include <utility>

namespace {

class SocketStream : public Stream {
public:
    explicit SocketStream(UniqueSocket socket) : socket_(std::move(socket)) {}

    SOCKET native_fd() const override { return socket_.get(); }

    ReadResult read(char* data, std::size_t size) override {
        if (!socket_.valid()) {
            return ReadResult(IoStatus::Error, 0U);
        }
        const int received = recv_bounded(socket_.get(), data, size, 0);
        if (received > 0) {
            return ReadResult(IoStatus::Ok, static_cast<std::size_t>(received));
        }
        if (received == 0) {
            return ReadResult(IoStatus::Eof, 0U);
        }
        if (would_block_error(last_socket_error())) {
            return ReadResult(IoStatus::WouldBlock, 0U);
        }
        return ReadResult(IoStatus::Error, 0U);
    }

    WriteResult write(const char* data, std::size_t size) override {
        if (!socket_.valid()) {
            return WriteResult(IoStatus::Error, 0U);
        }
        const int sent = send_bounded(socket_.get(), data, size, 0);
        if (sent > 0) {
            return WriteResult(IoStatus::Ok, static_cast<std::size_t>(sent));
        }
        if (sent < 0 && would_block_error(last_socket_error())) {
            return WriteResult(IoStatus::WouldBlock, 0U);
        }
        return WriteResult(IoStatus::Error, 0U);
    }

    void close() override { socket_.reset(); }

private:
    UniqueSocket socket_;
};

} // namespace

std::shared_ptr<Stream> make_socket_stream(UniqueSocket socket) {
    return std::shared_ptr<Stream>(new SocketStream(std::move(socket)));
}
