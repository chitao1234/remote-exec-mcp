#include "http/connection_transport.h"

#include <utility>

namespace {

class PlainConnectionTransport : public ConnectionTransport {
public:
    explicit PlainConnectionTransport(UniqueSocket socket)
        : owned_socket_(std::move(socket)), socket_(owned_socket_.get()) {}

    explicit PlainConnectionTransport(SOCKET socket) : owned_socket_(), socket_(socket) {}

    SOCKET native_socket() const override { return socket_; }

    int wait_readable(unsigned long timeout_ms) override {
        return wait_socket_readable(socket_, timeout_ms);
    }

    int read(char* data, std::size_t size) override { return recv_bounded(socket_, data, size, 0); }

    int write(const char* data, std::size_t size) override {
        return send_bounded(socket_, data, size, 0);
    }

    void set_timeout_ms(unsigned long timeout_ms) override {
        set_socket_timeout_ms(socket_, timeout_ms);
    }

    void shutdown() override { shutdown_socket(socket_); }

private:
    UniqueSocket owned_socket_;
    const SOCKET socket_;
};

} // namespace

std::shared_ptr<ConnectionTransport> make_plain_connection_transport(UniqueSocket socket) {
    return std::shared_ptr<ConnectionTransport>(new PlainConnectionTransport(std::move(socket)));
}

std::shared_ptr<ConnectionTransport> make_borrowed_connection_transport(SOCKET socket) {
    return std::shared_ptr<ConnectionTransport>(new PlainConnectionTransport(socket));
}
