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

inline bool is_valid_sequence_byte(unsigned char lead, std::size_t offset, unsigned char ch) {
    if (offset > 1U) {
        return is_continuation_byte(ch);
    }

    if (lead == 0xE0U) {
        return ch >= 0xA0U && ch <= 0xBFU;
    }
    if (lead >= 0xE1U && lead <= 0xECU) {
        return is_continuation_byte(ch);
    }
    if (lead == 0xEDU) {
        return ch >= 0x80U && ch <= 0x9FU;
    }
    if (lead >= 0xEEU && lead <= 0xEFU) {
        return is_continuation_byte(ch);
    }
    if (lead == 0xF0U) {
        return ch >= 0x90U && ch <= 0xBFU;
    }
    if (lead >= 0xF1U && lead <= 0xF3U) {
        return is_continuation_byte(ch);
    }
    if (lead == 0xF4U) {
        return ch >= 0x80U && ch <= 0x8FU;
    }

    return is_continuation_byte(ch);
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

        bool valid = true;
        bool incomplete = false;
        for (std::size_t offset = 1U; offset < expected; ++offset) {
            if (index + offset >= raw.size()) {
                incomplete = true;
                break;
            }

            if (!is_valid_sequence_byte(
                    ch,
                    offset,
                    static_cast<unsigned char>(raw[index + offset])
                )) {
                valid = false;
                break;
            }
        }

        if (incomplete) {
            if (!flush) {
                carry->assign(raw, index, raw.size() - index);
                break;
            }
            output += replacement_utf8();
            break;
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
