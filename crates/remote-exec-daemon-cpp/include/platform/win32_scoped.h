#pragma once

#include <windows.h>

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
