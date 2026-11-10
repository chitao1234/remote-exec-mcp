#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

// Bounded unread-output accumulator. When the cap is exceeded it keeps the
// beginning and the end of the stream so a reader still sees the command's
// opening and final output. total_bytes() reports everything appended since the
// last take(), including dropped bytes.
class BoundedTranscript {
public:
    explicit BoundedTranscript(std::size_t limit);

    void append(const std::string& chunk);
    std::string take();
    std::uint64_t total_bytes() const;
    bool empty() const;
    void clear();

private:
    std::string head_;
    std::string tail_;
    std::size_t head_limit_;
    std::size_t tail_limit_;
    std::uint64_t total_;
};
