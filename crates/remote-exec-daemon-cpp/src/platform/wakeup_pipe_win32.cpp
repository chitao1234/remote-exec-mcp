#ifdef _WIN32

#include <ws2tcpip.h>

#include "wakeup_pipe.h"

WakeupPipe::WakeupPipe() : read_end_(INVALID_SOCKET), write_end_(INVALID_SOCKET), signaled_(false) {
    SOCKET listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (listener == INVALID_SOCKET) {
        return;
    }

    sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = 0;

    if (bind(listener, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        closesocket(listener);
        return;
    }
    if (listen(listener, 1) != 0) {
        closesocket(listener);
        return;
    }

    int addr_len = sizeof(addr);
    if (getsockname(listener, reinterpret_cast<sockaddr*>(&addr), &addr_len) != 0) {
        closesocket(listener);
        return;
    }

    SOCKET connector = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (connector == INVALID_SOCKET) {
        closesocket(listener);
        return;
    }
    if (connect(connector, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        closesocket(connector);
        closesocket(listener);
        return;
    }

    SOCKET accepted = accept(listener, nullptr, nullptr);
    closesocket(listener);
    if (accepted == INVALID_SOCKET) {
        closesocket(connector);
        return;
    }

    read_end_ = accepted;
    write_end_ = connector;
}

WakeupPipe::~WakeupPipe() {
    if (read_end_ != INVALID_SOCKET) {
        closesocket(read_end_);
    }
    if (!signaled_ && write_end_ != INVALID_SOCKET) {
        closesocket(write_end_);
    }
}

void WakeupPipe::signal() {
    if (signaled_) {
        return;
    }
    signaled_ = true;
    if (write_end_ != INVALID_SOCKET) {
        const char byte = 1;
        send(write_end_, &byte, 1, 0);
        closesocket(write_end_);
        write_end_ = INVALID_SOCKET;
    }
}

SOCKET WakeupPipe::read_fd() const {
    return read_end_;
}

#endif
