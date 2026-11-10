#pragma once

#include <cstddef>
#include <string>

// Growable byte FIFO with a hard limit. The limit is enforced at append time so
// callers never have to trust an upstream length before allocating.
class ByteBuffer {
public:
    explicit ByteBuffer(std::size_t limit);

    bool append(const char* data, std::size_t size);
    void consume(std::size_t size);
    const char* data() const;
    std::size_t size() const;
    bool empty() const;
    std::size_t limit() const;
    void clear();
    std::string take();

private:
    void compact();

    std::string data_;
    std::size_t offset_;
    std::size_t limit_;
};
