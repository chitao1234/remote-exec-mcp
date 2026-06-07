#pragma once

#include <cstddef>
#include <stdexcept>
#include <string>

namespace win32_utf8 {

inline void append_utf8_code_point(std::string* out, unsigned int code_point) {
    if (code_point <= 0x7FU) {
        out->push_back(static_cast<char>(code_point));
        return;
    }
    if (code_point <= 0x7FFU) {
        out->push_back(static_cast<char>(0xC0U | (code_point >> 6U)));
        out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
        return;
    }
    if (code_point <= 0xFFFFU) {
        out->push_back(static_cast<char>(0xE0U | (code_point >> 12U)));
        out->push_back(static_cast<char>(0x80U | ((code_point >> 6U) & 0x3FU)));
        out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
        return;
    }

    out->push_back(static_cast<char>(0xF0U | (code_point >> 18U)));
    out->push_back(static_cast<char>(0x80U | ((code_point >> 12U) & 0x3FU)));
    out->push_back(static_cast<char>(0x80U | ((code_point >> 6U) & 0x3FU)));
    out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
}

inline bool is_high_surrogate(unsigned int code_unit) {
    return code_unit >= 0xD800U && code_unit <= 0xDBFFU;
}

inline bool is_low_surrogate(unsigned int code_unit) {
    return code_unit >= 0xDC00U && code_unit <= 0xDFFFU;
}

inline void append_wide_code_point(std::wstring* out, unsigned int code_point) {
    if (code_point <= 0xFFFFU) {
        if (is_high_surrogate(code_point) || is_low_surrogate(code_point)) {
            throw std::runtime_error("invalid UTF-8");
        }
        out->push_back(static_cast<wchar_t>(code_point));
        return;
    }
    if (code_point > 0x10FFFFU) {
        throw std::runtime_error("invalid UTF-8");
    }

    const unsigned int adjusted = code_point - 0x10000U;
    out->push_back(static_cast<wchar_t>(0xD800U + (adjusted >> 10U)));
    out->push_back(static_cast<wchar_t>(0xDC00U + (adjusted & 0x3FFU)));
}

inline unsigned char continuation_byte(const std::string& value, std::size_t index) {
    if (index >= value.size()) {
        throw std::runtime_error("invalid UTF-8");
    }
    const unsigned char byte = static_cast<unsigned char>(value[index]);
    if ((byte & 0xC0U) != 0x80U) {
        throw std::runtime_error("invalid UTF-8");
    }
    return byte;
}

inline std::wstring wide_from_utf8(const std::string& value) {
    std::wstring wide;
    for (std::size_t index = 0; index < value.size();) {
        const unsigned char first = static_cast<unsigned char>(value[index]);
        if (first <= 0x7FU) {
            wide.push_back(static_cast<wchar_t>(first));
            ++index;
            continue;
        }

        if (first >= 0xC2U && first <= 0xDFU) {
            const unsigned char second = continuation_byte(value, index + 1U);
            append_wide_code_point(&wide, ((first & 0x1FU) << 6U) | (second & 0x3FU));
            index += 2U;
            continue;
        }

        if (first >= 0xE0U && first <= 0xEFU) {
            const unsigned char second = continuation_byte(value, index + 1U);
            const unsigned char third = continuation_byte(value, index + 2U);
            if ((first == 0xE0U && second < 0xA0U) || (first == 0xEDU && second > 0x9FU)) {
                throw std::runtime_error("invalid UTF-8");
            }
            append_wide_code_point(
                &wide,
                ((first & 0x0FU) << 12U) | ((second & 0x3FU) << 6U) | (third & 0x3FU)
            );
            index += 3U;
            continue;
        }

        if (first >= 0xF0U && first <= 0xF4U) {
            const unsigned char second = continuation_byte(value, index + 1U);
            const unsigned char third = continuation_byte(value, index + 2U);
            const unsigned char fourth = continuation_byte(value, index + 3U);
            if ((first == 0xF0U && second < 0x90U) || (first == 0xF4U && second > 0x8FU)) {
                throw std::runtime_error("invalid UTF-8");
            }
            append_wide_code_point(
                &wide,
                ((first & 0x07U) << 18U) | ((second & 0x3FU) << 12U) | ((third & 0x3FU) << 6U)
                    | (fourth & 0x3FU)
            );
            index += 4U;
            continue;
        }

        throw std::runtime_error("invalid UTF-8");
    }
    return wide;
}

inline std::string replacement_utf8() {
    return "\xEF\xBF\xBD";
}

inline std::string utf8_from_wide(const std::wstring& wide) {
    std::string utf8;
    for (std::size_t index = 0; index < wide.size(); ++index) {
        const unsigned int code_unit = static_cast<unsigned int>(wide[index]);
        if (is_high_surrogate(code_unit)) {
            if (index + 1U < wide.size()) {
                const unsigned int next = static_cast<unsigned int>(wide[index + 1U]);
                if (is_low_surrogate(next)) {
                    const unsigned int code_point =
                        0x10000U + ((code_unit - 0xD800U) << 10U) + (next - 0xDC00U);
                    append_utf8_code_point(&utf8, code_point);
                    ++index;
                    continue;
                }
            }
            utf8 += replacement_utf8();
            continue;
        }
        if (is_low_surrogate(code_unit)) {
            utf8 += replacement_utf8();
            continue;
        }
        append_utf8_code_point(&utf8, code_unit);
    }
    return utf8;
}

} // namespace win32_utf8
