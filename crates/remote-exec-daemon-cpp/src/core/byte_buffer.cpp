#include "core/byte_buffer.h"

namespace {

const std::size_t COMPACT_MIN_BYTES = 8192U;

} // namespace

ByteBuffer::ByteBuffer(std::size_t limit) : offset_(0), limit_(limit) {
}

bool ByteBuffer::append(const char* data, std::size_t size) {
    if (size == 0U) {
        return true;
    }
    const std::size_t used = data_.size() - offset_;
    if (used > limit_ || size > limit_ - used) {
        return false;
    }
    data_.append(data, size);
    return true;
}

void ByteBuffer::consume(std::size_t size) {
    if (size >= data_.size() - offset_) {
        clear();
        return;
    }
    offset_ += size;
    compact();
}

const char* ByteBuffer::data() const {
    return data_.empty() ? "" : data_.data() + offset_;
}

std::size_t ByteBuffer::size() const {
    return data_.size() - offset_;
}

bool ByteBuffer::empty() const {
    return size() == 0U;
}

std::size_t ByteBuffer::limit() const {
    return limit_;
}

void ByteBuffer::clear() {
    data_.clear();
    offset_ = 0U;
}

std::string ByteBuffer::take() {
    std::string out = data_.substr(offset_);
    clear();
    return out;
}

void ByteBuffer::compact() {
    if (offset_ >= COMPACT_MIN_BYTES && offset_ * 2U >= data_.size()) {
        data_.erase(0, offset_);
        offset_ = 0U;
    }
}
