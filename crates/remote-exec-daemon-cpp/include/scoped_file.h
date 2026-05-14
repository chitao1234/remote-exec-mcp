#pragma once

#include <cstdio>

class ScopedFile {
public:
    ScopedFile() : file_(NULL) {}
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
        return file_ != NULL;
    }

    FILE* release() {
        FILE* file = file_;
        file_ = NULL;
        return file;
    }

    int close() {
        if (file_ == NULL) {
            return 0;
        }
        FILE* file = file_;
        file_ = NULL;
        return std::fclose(file);
    }

    void reset(FILE* file = NULL) {
        if (file_ != NULL) {
            std::fclose(file_);
        }
        file_ = file;
    }

private:
    FILE* file_;
};
