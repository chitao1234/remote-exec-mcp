#include "exec/bounded_transcript.h"

#include <algorithm>

BoundedTranscript::BoundedTranscript(std::size_t limit)
    : head_limit_(limit / 2U), tail_limit_(limit - limit / 2U), total_(0U) {
}

void BoundedTranscript::append(const std::string& chunk) {
    if (chunk.empty()) {
        return;
    }
    total_ += static_cast<std::uint64_t>(chunk.size());

    if (head_.size() < head_limit_) {
        const std::size_t take = std::min(head_limit_ - head_.size(), chunk.size());
        head_.append(chunk.data(), take);
        if (take == chunk.size()) {
            return;
        }
        tail_.append(chunk.data() + take, chunk.size() - take);
    } else {
        tail_.append(chunk);
    }

    if (tail_limit_ == 0U) {
        tail_.clear();
        return;
    }
    if (tail_.size() > tail_limit_) {
        tail_.erase(0, tail_.size() - tail_limit_);
    }
}

std::string BoundedTranscript::take() {
    std::string out = head_;
    out += tail_;
    clear();
    return out;
}

std::uint64_t BoundedTranscript::total_bytes() const {
    return total_;
}

bool BoundedTranscript::empty() const {
    return head_.empty() && tail_.empty();
}

void BoundedTranscript::clear() {
    head_.clear();
    tail_.clear();
    total_ = 0U;
}
