#include "output_renderer.h"

#include <algorithm>
#include <sstream>

namespace {

const std::size_t BYTES_PER_TOKEN = 4U;

unsigned long count_lines(const std::string& output) {
    if (output.empty()) {
        return 0UL;
    }

    unsigned long lines = 1UL;
    for (std::string::const_iterator it = output.begin(); it != output.end(); ++it) {
        if (*it == '\n') {
            ++lines;
        }
    }
    if (output[output.size() - 1] == '\n') {
        --lines;
    }
    return lines;
}

bool is_utf8_continuation_byte(unsigned char byte) {
    return (byte & 0xC0U) == 0x80U;
}

std::size_t floor_char_boundary(const std::string& output, std::size_t max_bytes) {
    std::size_t index = std::min(max_bytes, output.size());
    while (index > 0U && index < output.size() &&
           is_utf8_continuation_byte(static_cast<unsigned char>(output[index]))) {
        --index;
    }
    return index;
}

std::size_t ceil_char_boundary(const std::string& output, std::size_t min_bytes) {
    std::size_t index = std::min(min_bytes, output.size());
    while (index < output.size() && is_utf8_continuation_byte(static_cast<unsigned char>(output[index]))) {
        ++index;
    }
    return index;
}

std::size_t suffix_start_for_budget(const std::string& output, std::size_t max_bytes) {
    if (max_bytes >= output.size()) {
        return 0U;
    }
    return ceil_char_boundary(output, output.size() - max_bytes);
}

std::string truncation_prefix(unsigned long line_count) {
    std::ostringstream out;
    out << "Total output lines: " << line_count << "\n\n";
    return out.str();
}

std::string truncation_marker(unsigned long truncated_tokens) {
    std::ostringstream out;
    out << "\xE2\x80\xA6" << truncated_tokens << " tokens truncated" << "\xE2\x80\xA6";
    return out.str();
}

} // namespace

unsigned long approximate_output_token_count(std::size_t bytes) {
    if (bytes == 0U) {
        return 0UL;
    }
    return static_cast<unsigned long>((bytes + BYTES_PER_TOKEN - 1U) / BYTES_PER_TOKEN);
}

std::string render_output(const std::string& output, unsigned long max_output_tokens) {
    if (max_output_tokens == 0UL) {
        return std::string();
    }

    const std::size_t max_output_bytes = static_cast<std::size_t>(max_output_tokens) * BYTES_PER_TOKEN;
    if (output.size() <= max_output_bytes) {
        return output;
    }

    const std::string prefix = truncation_prefix(count_lines(output));
    unsigned long truncated_tokens = approximate_output_token_count(output.size());
    for (;;) {
        const std::string marker = truncation_marker(truncated_tokens);
        if (max_output_bytes <= prefix.size() + marker.size()) {
            return prefix + marker;
        }

        const std::size_t payload_budget = max_output_bytes - prefix.size() - marker.size();
        const std::size_t head_budget = payload_budget / 2U;
        const std::size_t tail_budget = payload_budget - head_budget;
        const std::size_t head_end = floor_char_boundary(output, head_budget);
        const std::size_t tail_start = std::max(head_end, suffix_start_for_budget(output, tail_budget));
        const unsigned long next_truncated_tokens = approximate_output_token_count(tail_start - head_end);

        if (next_truncated_tokens == truncated_tokens) {
            return prefix + output.substr(0, head_end) + marker + output.substr(tail_start);
        }

        truncated_tokens = next_truncated_tokens;
    }
}
