#pragma once

#include <cstddef>
#include <string>

namespace utf8_stream_decode {

inline std::string replacement_utf8() {
    return "\xEF\xBF\xBD";
}

inline bool is_continuation_byte(unsigned char ch) {
    return (ch & 0xC0U) == 0x80U;
}

inline std::string decode_utf8_stream_chunk(
    std::string* carry,
    const std::string& raw_chunk,
    bool flush
) {
    std::string raw = *carry;
    raw += raw_chunk;
    carry->clear();

    std::string output;
    for (std::size_t index = 0; index < raw.size();) {
        const unsigned char ch = static_cast<unsigned char>(raw[index]);
        if (ch < 0x80U) {
            output.push_back(static_cast<char>(ch));
            ++index;
            continue;
        }

        std::size_t expected = 0U;
        if (ch >= 0xC2U && ch <= 0xDFU) {
            expected = 2U;
        } else if (ch >= 0xE0U && ch <= 0xEFU) {
            expected = 3U;
        } else if (ch >= 0xF0U && ch <= 0xF4U) {
            expected = 4U;
        } else {
            output += replacement_utf8();
            ++index;
            continue;
        }

        if (index + expected > raw.size()) {
            if (!flush) {
                carry->assign(raw, index, raw.size() - index);
                break;
            }
            output += replacement_utf8();
            break;
        }

        bool valid = true;
        for (std::size_t offset = 1U; offset < expected; ++offset) {
            if (!is_continuation_byte(static_cast<unsigned char>(raw[index + offset]))) {
                valid = false;
                break;
            }
        }

        if (!valid) {
            output += replacement_utf8();
            ++index;
            continue;
        }

        output.append(raw, index, expected);
        index += expected;
    }

    return output;
}

} // namespace utf8_stream_decode
