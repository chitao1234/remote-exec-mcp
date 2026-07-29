#include "http/connection_transport.h"

namespace {

class PlainConnectionTransport : public ConnectionTransport {
public:
    PlainConnectionTransport(SOCKET socket, bool owns_socket)
        : socket_(socket), owns_socket_(owns_socket) {}

    ~PlainConnectionTransport() {
        if (owns_socket_ && socket_ != INVALID_SOCKET) {
            close_socket(socket_);
        }
    }

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
    SOCKET socket_;
    bool owns_socket_;
};

} // namespace

std::shared_ptr<ConnectionTransport> make_plain_connection_transport(UniqueSocket socket) {
    const SOCKET raw_socket = socket.release();
    return std::shared_ptr<ConnectionTransport>(new PlainConnectionTransport(raw_socket, true));
}

std::shared_ptr<ConnectionTransport> make_borrowed_connection_transport(SOCKET socket) {
    return std::shared_ptr<ConnectionTransport>(new PlainConnectionTransport(socket, false));
}
