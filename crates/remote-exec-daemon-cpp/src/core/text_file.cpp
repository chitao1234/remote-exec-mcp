#include "core/text_file.h"

#include "core/encoding_detect.h"
#include "core/legacy_encoding.h"

namespace {

// Encoding identifiers. utf-8 is the implicit encoding for the fast path;
// the remaining values are produced by detection.
const std::uint32_t ENCODING_UTF8 = 0U;
const std::uint32_t ENCODING_UTF16LE = 1U;
const std::uint32_t ENCODING_UTF16BE = 2U;
// A legacy encoding detected statistically. The concrete encoding is carried
// in DetectedTextEncoding::legacy.
const std::uint32_t ENCODING_LEGACY = 3U;

const char UTF8_BOM[] = "\xEF\xBB\xBF";
const std::size_t UTF8_BOM_LEN = 3U;
const std::size_t UTF16_BOM_LEN = 2U;

void append_utf8_code_point(std::uint32_t code_point, std::string* out) {
    if (code_point <= 0x7FU) {
        out->push_back(static_cast<char>(code_point));
        return;
    }
    if (code_point <= 0x7FFU) {
        out->push_back(static_cast<char>(0xC0U | (code_point >> 6)));
        out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
        return;
    }
    if (code_point <= 0xFFFFU) {
        out->push_back(static_cast<char>(0xE0U | (code_point >> 12)));
        out->push_back(static_cast<char>(0x80U | ((code_point >> 6) & 0x3FU)));
        out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
        return;
    }
    out->push_back(static_cast<char>(0xF0U | (code_point >> 18)));
    out->push_back(static_cast<char>(0x80U | ((code_point >> 12) & 0x3FU)));
    out->push_back(static_cast<char>(0x80U | ((code_point >> 6) & 0x3FU)));
    out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
}

// Strict UTF-8 validation. Rejects overlong forms, surrogates, and values
// above U+10FFFF, matching the Rust daemon's String::from_utf8 strictness.
bool decode_utf8(const std::string& bytes, std::string* out) {
    out->clear();
    out->reserve(bytes.size());
    std::size_t i = 0U;
    while (i < bytes.size()) {
        const unsigned char first = static_cast<unsigned char>(bytes[i]);
        if (first < 0x80U) {
            out->push_back(static_cast<char>(first));
            i += 1U;
            continue;
        }

        std::size_t width = 0U;
        std::uint32_t code_point = 0U;
        std::uint32_t lowest = 0U;
        if ((first & 0xE0U) == 0xC0U) {
            width = 2U;
            code_point = first & 0x1FU;
            lowest = 0x80U;
        } else if ((first & 0xF0U) == 0xE0U) {
            width = 3U;
            code_point = first & 0x0FU;
            lowest = 0x800U;
        } else if ((first & 0xF8U) == 0xF0U) {
            width = 4U;
            code_point = first & 0x07U;
            lowest = 0x10000U;
        } else {
            return false;
        }

        if (i + width > bytes.size()) {
            return false;
        }
        for (std::size_t k = 1U; k < width; ++k) {
            const unsigned char continuation = static_cast<unsigned char>(bytes[i + k]);
            if ((continuation & 0xC0U) != 0x80U) {
                return false;
            }
            code_point = (code_point << 6) | (continuation & 0x3FU);
        }

        if (code_point < lowest || code_point > 0x10FFFFU) {
            return false;
        }
        if (code_point >= 0xD800U && code_point <= 0xDFFFU) {
            return false;
        }

        out->append(bytes, i, width);
        i += width;
    }
    return true;
}

std::string encode_utf16(const std::string& text, bool little_endian) {
    std::string out;
    out.reserve(text.size() * 2U);

    std::size_t i = 0U;
    while (i < text.size()) {
        std::uint32_t code_point = 0U;
        const unsigned char first = static_cast<unsigned char>(text[i]);
        std::size_t width = 1U;
        if (first < 0x80U) {
            code_point = first;
        } else if ((first & 0xE0U) == 0xC0U) {
            width = 2U;
            code_point = first & 0x1FU;
        } else if ((first & 0xF0U) == 0xE0U) {
            width = 3U;
            code_point = first & 0x0FU;
        } else if ((first & 0xF8U) == 0xF0U) {
            width = 4U;
            code_point = first & 0x07U;
        } else {
            throw TextFileError(TextFileError::ENCODE_FAILED, "updated text is not valid UTF-8");
        }
        if (i + width > text.size()) {
            throw TextFileError(TextFileError::ENCODE_FAILED, "updated text is not valid UTF-8");
        }
        for (std::size_t k = 1U; k < width; ++k) {
            code_point = (code_point << 6) | (static_cast<unsigned char>(text[i + k]) & 0x3FU);
        }
        i += width;

        std::uint32_t units[2] = {0U, 0U};
        std::size_t unit_count = 1U;
        if (code_point <= 0xFFFFU) {
            units[0] = code_point;
        } else {
            const std::uint32_t adjusted = code_point - 0x10000U;
            units[0] = 0xD800U + (adjusted >> 10);
            units[1] = 0xDC00U + (adjusted & 0x3FFU);
            unit_count = 2U;
        }

        for (std::size_t u = 0U; u < unit_count; ++u) {
            const std::uint16_t unit = static_cast<std::uint16_t>(units[u]);
            if (little_endian) {
                out.push_back(static_cast<char>(unit & 0xFFU));
                out.push_back(static_cast<char>((unit >> 8) & 0xFFU));
            } else {
                out.push_back(static_cast<char>((unit >> 8) & 0xFFU));
                out.push_back(static_cast<char>(unit & 0xFFU));
            }
        }
    }
    return out;
}

// Decode UTF-16 into UTF-8. `bytes` excludes any byte-order mark.
bool decode_utf16(const std::string& bytes, bool little_endian, std::string* out) {
    if (bytes.size() % 2U != 0U) {
        return false;
    }

    out->clear();
    out->reserve(bytes.size());
    for (std::size_t i = 0U; i < bytes.size(); i += 2U) {
        const unsigned char low_byte = static_cast<unsigned char>(bytes[i]);
        const unsigned char high_byte = static_cast<unsigned char>(bytes[i + 1U]);
        std::uint32_t unit = 0U;
        if (little_endian) {
            unit =
                static_cast<std::uint32_t>(low_byte) | (static_cast<std::uint32_t>(high_byte) << 8);
        } else {
            unit =
                (static_cast<std::uint32_t>(low_byte) << 8) | static_cast<std::uint32_t>(high_byte);
        }

        if (unit >= 0xD800U && unit <= 0xDBFFU) {
            if (i + 3U >= bytes.size()) {
                return false;
            }
            const unsigned char next_low = static_cast<unsigned char>(bytes[i + 2U]);
            const unsigned char next_high = static_cast<unsigned char>(bytes[i + 3U]);
            std::uint32_t trail = 0U;
            if (little_endian) {
                trail = static_cast<std::uint32_t>(next_low)
                        | (static_cast<std::uint32_t>(next_high) << 8);
            } else {
                trail = (static_cast<std::uint32_t>(next_low) << 8)
                        | static_cast<std::uint32_t>(next_high);
            }
            if (trail < 0xDC00U || trail > 0xDFFFU) {
                return false;
            }
            const std::uint32_t code_point =
                0x10000U + ((unit - 0xD800U) << 10) + (trail - 0xDC00U);
            append_utf8_code_point(code_point, out);
            i += 2U;
            continue;
        }
        if (unit >= 0xDC00U && unit <= 0xDFFFU) {
            return false;
        }

        append_utf8_code_point(unit, out);
    }
    return true;
}

// Guesses UTF-16 for BOM-less content. NUL padding is the only signal strong
// enough to act on without a full statistical detector; plain text never
// carries NUL bytes in either byte position.
bool looks_like_utf16_without_bom(const std::string& bytes, bool* little_endian) {
    if (bytes.size() < 4U || bytes.size() % 2U != 0U) {
        return false;
    }

    std::size_t odd_nuls = 0U;
    std::size_t even_nuls = 0U;
    const std::size_t limit = bytes.size() < 1024U ? bytes.size() : 1024U;
    for (std::size_t i = 0U; i + 1U < limit; i += 2U) {
        if (bytes[i] == '\0') {
            even_nuls += 1U;
        }
        if (bytes[i + 1U] == '\0') {
            odd_nuls += 1U;
        }
    }

    const std::size_t pairs = limit / 2U;
    if (pairs == 0U) {
        return false;
    }
    // Latin and CJK text in UTF-16LE leaves every odd byte clear; UTF-16BE
    // leaves every even byte clear.
    if (odd_nuls * 4U >= pairs * 3U && even_nuls == 0U) {
        *little_endian = true;
        return true;
    }
    if (even_nuls * 4U >= pairs * 3U && odd_nuls == 0U) {
        *little_endian = false;
        return true;
    }
    return false;
}

// Full detection stage. Handles the byte-order marks and BOM-less UTF-16; the
// statistical legacy-encoding stage is layered on top of this.
bool detect_and_decode(const std::string& bytes, std::string* text, DetectedTextEncoding* out) {
    if (bytes.size() >= UTF8_BOM_LEN
        && bytes.compare(0U, UTF8_BOM_LEN, UTF8_BOM, UTF8_BOM_LEN) == 0) {
        const std::string content = bytes.substr(UTF8_BOM_LEN);
        if (!decode_utf8(content, text)) {
            return false;
        }
        out->kind_id = ENCODING_UTF8;
        out->bom.assign(bytes, 0U, UTF8_BOM_LEN);
        out->bom_length = UTF8_BOM_LEN;
        return true;
    }

    if (bytes.size() >= UTF16_BOM_LEN) {
        const unsigned char first = static_cast<unsigned char>(bytes[0]);
        const unsigned char second = static_cast<unsigned char>(bytes[1]);
        bool little_endian = false;
        bool has_bom = false;
        if (first == 0xFFU && second == 0xFEU) {
            little_endian = true;
            has_bom = true;
            out->kind_id = ENCODING_UTF16LE;
        } else if (first == 0xFEU && second == 0xFFU) {
            little_endian = false;
            has_bom = true;
            out->kind_id = ENCODING_UTF16BE;
        }

        if (has_bom) {
            const std::string content = bytes.substr(UTF16_BOM_LEN);
            if (!decode_utf16(content, little_endian, text)) {
                return false;
            }
            out->bom.assign(bytes, 0U, UTF16_BOM_LEN);
            out->bom_length = UTF16_BOM_LEN;
            return true;
        }
    }

    bool little_endian = false;
    if (looks_like_utf16_without_bom(bytes, &little_endian)) {
        if (!decode_utf16(bytes, little_endian, text)) {
            return false;
        }
        out->kind_id = little_endian ? ENCODING_UTF16LE : ENCODING_UTF16BE;
        // A BOM-less file stays BOM-less; only the byte order is remembered.
        out->bom.clear();
        out->bom_length = 0U;
        return true;
    }

    // Statistical detection for legacy encodings. The caller verifies the
    // result re-encodes to exactly these bytes, so a wrong guess is rejected
    // rather than silently applied.
    LegacyEncoding legacy;
    if (detect_legacy_encoding(bytes, &legacy)) {
        if (!legacy_encoding_decode(legacy, bytes, text)) {
            return false;
        }
        out->kind_id = ENCODING_LEGACY;
        out->legacy = legacy;
        out->bom.clear();
        out->bom_length = 0U;
        return true;
    }

    return false;
}

} // namespace

bool looks_like_text(const std::string& text) {
    if (text.find('\0') != std::string::npos) {
        return false;
    }

    std::size_t suspicious_controls = 0U;
    std::size_t char_count = 0U;
    std::size_t i = 0U;
    while (i < text.size()) {
        const unsigned char byte = static_cast<unsigned char>(text[i]);
        std::size_t width = 1U;
        if (byte < 0x80U) {
            if (byte < 0x20U && byte != '\n' && byte != '\r' && byte != '\t' && byte != '\f') {
                suspicious_controls += 1U;
            }
        } else if ((byte & 0xE0U) == 0xC0U) {
            width = 2U;
        } else if ((byte & 0xF0U) == 0xE0U) {
            width = 3U;
        } else if ((byte & 0xF8U) == 0xF0U) {
            width = 4U;
        }
        if (i + width > text.size()) {
            width = 1U;
        }
        char_count += 1U;
        i += width;
    }

    // Mirrors the Rust daemon's threshold: all-clean text passes, and up to
    // one suspicious control byte in twenty is tolerated.
    return suspicious_controls == 0U
           || suspicious_controls * 20U <= (char_count == 0U ? 1U : char_count);
}

TextFile TextFile::from_bytes(const std::string& bytes, bool allow_autodetect) {
    TextFile file;
    std::string decoded;
    if (decode_utf8(bytes, &decoded)) {
        // Deliberately not routed through detection: valid UTF-8 wins over a
        // byte-order mark guess, matching the Rust daemon, which only reaches
        // detection after String::from_utf8 fails.
        file.text_ = decoded;
        file.encoding_.kind_id = ENCODING_UTF8;
        file.encoding_.bom.clear();
        file.encoding_.bom_length = 0U;
        return file;
    }

    if (!allow_autodetect) {
        throw TextFileError(
            TextFileError::DECODE_FAILED,
            "target file is not valid utf-8 text; enable "
            "experimental_apply_patch_target_encoding_autodetect to decode other encodings"
        );
    }

    DetectedTextEncoding detected;
    if (!detect_and_decode(bytes, &decoded, &detected)) {
        throw TextFileError(
            TextFileError::DECODE_FAILED,
            "target file encoding could not be detected"
        );
    }

    if (!looks_like_text(decoded)) {
        throw TextFileError(TextFileError::BINARY, "detected content does not look like text");
    }

    // Round-trip check. A decoding that cannot be reproduced byte for byte
    // would silently rewrite the file on an edit, so it is rejected here.
    TextFile candidate;
    candidate.text_ = decoded;
    candidate.encoding_ = detected;
    if (candidate.encode(decoded) != bytes) {
        throw TextFileError(
            TextFileError::DECODE_FAILED,
            "detected content did not round-trip cleanly"
        );
    }

    return candidate;
}

std::string TextFile::encode(const std::string& text) const {
    std::string encoded;
    switch (encoding_.kind_id) {
    case ENCODING_UTF8:
        encoded = text;
        break;
    case ENCODING_UTF16LE:
        encoded = encode_utf16(text, true);
        break;
    case ENCODING_UTF16BE:
        encoded = encode_utf16(text, false);
        break;
    case ENCODING_LEGACY:
        if (!legacy_encoding_encode(encoding_.legacy, text, &encoded)) {
            throw TextFileError(
                TextFileError::ENCODE_FAILED,
                std::string("updated text cannot be represented in detected ")
                    + legacy_encoding_name(encoding_.legacy) + " encoding"
            );
        }
        break;
    default:
        throw TextFileError(TextFileError::ENCODE_FAILED, "detected encoding cannot be re-encoded");
    }

    if (encoding_.bom.empty()) {
        return encoded;
    }
    return encoding_.bom + encoded;
}
