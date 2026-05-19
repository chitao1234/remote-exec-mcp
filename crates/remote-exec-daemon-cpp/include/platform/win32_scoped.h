#pragma once

#include <stdexcept>

#include <windows.h>
#include <winsock2.h>

class UniqueHandle {
public:
    UniqueHandle() : handle_(nullptr) {}
    explicit UniqueHandle(HANDLE handle) : handle_(handle) {}

    ~UniqueHandle() { reset(); }

    UniqueHandle(UniqueHandle&& other) : handle_(other.release()) {}

    UniqueHandle& operator=(UniqueHandle&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    UniqueHandle(const UniqueHandle&) = delete;
    UniqueHandle& operator=(const UniqueHandle&) = delete;

    HANDLE get() const { return handle_; }

    bool valid() const { return handle_ != nullptr && handle_ != INVALID_HANDLE_VALUE; }

    HANDLE release() {
        const HANDLE released = handle_;
        handle_ = nullptr;
        return released;
    }

    void reset(HANDLE handle = nullptr) {
        if (valid()) {
            CloseHandle(handle_);
        }
        handle_ = handle;
    }

private:
    HANDLE handle_;
};

class UniqueSocket {
public:
    UniqueSocket() : socket_(INVALID_SOCKET) {}
    explicit UniqueSocket(SOCKET socket) : socket_(socket) {}

    ~UniqueSocket() { reset(); }

    UniqueSocket(UniqueSocket&& other) : socket_(other.release()) {}

    UniqueSocket& operator=(UniqueSocket&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    UniqueSocket(const UniqueSocket&) = delete;
    UniqueSocket& operator=(const UniqueSocket&) = delete;

    SOCKET get() const { return socket_; }

    bool valid() const { return socket_ != INVALID_SOCKET; }

    SOCKET release() {
        const SOCKET released = socket_;
        socket_ = INVALID_SOCKET;
        return released;
    }

    void reset(SOCKET socket = INVALID_SOCKET) {
        if (valid()) {
            closesocket(socket_);
        }
        socket_ = socket;
    }

private:
    SOCKET socket_;
};

class WinsockSession {
public:
    WinsockSession() {
        WSADATA wsa_data;
        if (WSAStartup(MAKEWORD(2, 2), &wsa_data) != 0) {
            throw std::runtime_error("WSAStartup failed");
        }
    }

    ~WinsockSession() { WSACleanup(); }

    WinsockSession(const WinsockSession&) = delete;
    WinsockSession& operator=(const WinsockSession&) = delete;
};
