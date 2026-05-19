#pragma once

#include <cstdio>

class ScopedFile {
public:
    ScopedFile() : file_(nullptr) {}
    explicit ScopedFile(FILE* file) : file_(file) {}

    ~ScopedFile() {
        reset();
    }

    ScopedFile(ScopedFile&& other) : file_(other.release()) {}

    ScopedFile& operator=(ScopedFile&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    ScopedFile(const ScopedFile&) = delete;
    ScopedFile& operator=(const ScopedFile&) = delete;

    FILE* get() const {
        return file_;
    }

    bool valid() const {
        return file_ != nullptr;
    }

    FILE* release() {
        FILE* file = file_;
        file_ = nullptr;
        return file;
    }

    int close() {
        if (file_ == nullptr) {
            return 0;
        }
        FILE* file = file_;
        file_ = nullptr;
        return std::fclose(file);
    }

    void reset(FILE* file = nullptr) {
        if (file_ != nullptr) {
            std::fclose(file_);
        }
        file_ = file;
    }

private:
    FILE* file_;
};
